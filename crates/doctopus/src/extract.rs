//! Saving parts to disk: `doctopus extract` and `doctopus read --extract`.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use doctopus_core::render::{describe_part, shell_quote};
use doctopus_core::{Address, Document, Part};
use serde::Serialize;

use crate::cli::{ExtractArgs, stem};
use crate::failure::Failure;
use crate::load::{engine, load};
use crate::output::print;
use crate::paths;

/// What was saved.
#[derive(Debug, Default, Serialize)]
pub(crate) struct Report {
    /// Files written, depth first.
    pub(crate) files: Vec<Saved>,
    /// Parts that have no payload to save.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) skipped: Vec<Skipped>,
}

/// One saved part.
#[derive(Debug, Serialize)]
pub(crate) struct Saved {
    /// Part ids from the extracted document (`emb1/img2`); empty for the
    /// addressed part itself.
    pub(crate) path: String,
    /// The part's address, usable as a command argument.
    pub(crate) address: String,
    /// Where it was written (absolute in `extract`'s report).
    pub(crate) file: PathBuf,
    /// Format id when doctopus can read the part as a document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) format: Option<&'static str>,
    /// Media type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) content_type: Option<String>,
    /// Size in bytes.
    pub(crate) size: u64,
    /// One-line description.
    pub(crate) summary: String,
}

/// A part with nothing to save.
#[derive(Debug, Serialize)]
pub(crate) struct Skipped {
    pub(crate) address: String,
    pub(crate) reason: String,
}

/// `doctopus extract`.
pub(crate) fn run(args: &ExtractArgs, max_depth: u32) -> Result<(), Failure> {
    let address = Address::parse(&args.address);
    let loaded = load(&engine(max_depth, false), &address)?;
    // Without -o, next to the file.
    let dir = args
        .output
        .clone()
        .unwrap_or_else(|| paths::beside(&address).join(format!("{}.parts", stem(&address))));
    // Saving fails only to write.
    let mut report = save(loaded.document(), loaded.part(), &address, &dir)
        .map_err(|failure| failure.hint("save somewhere else with -o"))?;
    let dir = paths::shown(&dir);
    for saved in &mut report.files {
        saved.file = paths::shown(&saved.file);
    }
    if args.json {
        #[derive(Serialize)]
        struct Manifest<'a> {
            source: String,
            directory: &'a Path,
            #[serde(flatten)]
            report: &'a Report,
        }
        let manifest = Manifest {
            source: address.to_string(),
            directory: &dir,
            report: &report,
        };
        let json = serde_json::to_string_pretty(&manifest).unwrap_or_default();
        return print(&(json + "\n"));
    }
    print(&describe(&report, &address, &dir))
}

/// Save the addressed part (if any) and every part of `document`, nested
/// ones included, under `dir`.
pub(crate) fn save(
    document: Option<&Document>,
    part: Option<&Part>,
    address: &Address,
    dir: &Path,
) -> Result<Report, Failure> {
    let mut report = Report::default();
    let mut saver = Saver {
        root: address,
        report: &mut report,
    };
    let mut used = HashSet::new();
    if let Some(part) = part {
        saver.part(part, "", dir, &mut used)?;
    }
    if let Some(document) = document {
        saver.document(document, &mut Vec::new(), dir, used)?;
    }
    Ok(report)
}

struct Saver<'a> {
    root: &'a Address,
    report: &'a mut Report,
}

impl Saver<'_> {
    /// Save the parts of `doc`, found at `path` below the root, into `dir`.
    /// `used` holds the names already taken in `dir`.
    fn document(
        &mut self,
        doc: &Document,
        path: &mut Vec<String>,
        dir: &Path,
        mut used: HashSet<String>,
    ) -> Result<(), Failure> {
        // Nested documents get a directory named after their id; keep
        // those names free for them.
        for part in &doc.parts {
            if has_parts(part) {
                used.insert(part.id.as_str().to_lowercase());
            }
        }
        for part in &doc.parts {
            path.push(part.id.to_string());
            self.part(part, &path.join("/"), dir, &mut used)?;
            if let Some(child) = part.document.as_deref().filter(|_| has_parts(part)) {
                let sub = dir.join(part.id.as_str());
                self.document(child, path, &sub, HashSet::new())?;
            }
            path.pop();
        }
        Ok(())
    }

    fn part(
        &mut self,
        part: &Part,
        path: &str,
        dir: &Path,
        used: &mut HashSet<String>,
    ) -> Result<(), Failure> {
        let mut address = self.root.clone();
        address.parts.extend(
            path.split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        );
        if part.data.is_empty() {
            self.report.skipped.push(Skipped {
                address: address.to_string(),
                reason: empty_reason(part),
            });
            return Ok(());
        }
        std::fs::create_dir_all(dir).map_err(|err| Failure::write(dir, &err))?;
        let file = dir.join(unique_name(used, part.file_name()));
        std::fs::write(&file, &part.data).map_err(|err| Failure::write(&file, &err))?;
        self.report.files.push(Saved {
            path: path.to_string(),
            address: address.to_string(),
            file,
            format: part.document.as_ref().map(|d| d.format.id),
            content_type: part.content_type.clone(),
            size: part.data.len() as u64,
            summary: describe_part(part),
        });
        Ok(())
    }
}

/// Why a part without data has nothing to save.
pub(crate) fn empty_reason(part: &Part) -> String {
    match &part.link {
        Some(link) => format!("linked, not embedded: {link}"),
        None => "no content".into(),
    }
}

fn has_parts(part: &Part) -> bool {
    part.document.as_ref().is_some_and(|d| !d.parts.is_empty())
}

/// `name`, or `name-2`, `name-3`... if it is taken. Names compare without
/// case, as they do on Windows and macOS file systems.
pub(crate) fn unique_name(used: &mut HashSet<String>, name: String) -> String {
    if used.insert(name.to_lowercase()) {
        return name;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name.as_str(), String::new()),
    };
    let mut n = 2;
    loop {
        let candidate = format!("{stem}-{n}{ext}");
        if used.insert(candidate.to_lowercase()) {
            return candidate;
        }
        n += 1;
    }
}

/// The report as YAML-style text.
fn describe(report: &Report, address: &Address, dir: &Path) -> String {
    let mut out = String::new();
    if report.files.is_empty() && report.skipped.is_empty() {
        let _ = writeln!(
            out,
            "nothing to extract: {address} has no images, embedded documents or attachments"
        );
        return out;
    }
    let _ = writeln!(
        out,
        "directory: {}  # the paths below are relative to it",
        shell_quote(&dir.display().to_string())
    );
    let _ = writeln!(out, "files: # {}", report.files.len());
    for saved in &report.files {
        let _ = writeln!(
            out,
            "  - {}  # {}: {}",
            shell_quote(&paths::within(dir, &saved.file).display().to_string()),
            saved.address,
            saved.summary
        );
    }
    if !report.skipped.is_empty() {
        out.push_str("skipped:\n");
        for skipped in &report.skipped {
            let _ = writeln!(out, "  - {}: {}", skipped.address, skipped.reason);
        }
    }
    if let Some(doc) = report.files.iter().find(|s| s.format.is_some()) {
        let _ = writeln!(
            out,
            "next: doctopus read {}  # read an embedded document on its own",
            shell_quote(&doc.address)
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_unique_without_case() {
        let mut used = HashSet::new();
        assert_eq!(unique_name(&mut used, "image1.png".into()), "image1.png");
        assert_eq!(unique_name(&mut used, "Image1.PNG".into()), "Image1-2.PNG");
        assert_eq!(unique_name(&mut used, "image1.png".into()), "image1-3.png");
        assert_eq!(unique_name(&mut used, "README".into()), "README");
        assert_eq!(unique_name(&mut used, "readme".into()), "readme-2");
    }
}
