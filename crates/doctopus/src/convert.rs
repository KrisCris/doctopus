//! `doctopus convert`: documents as Markdown files that are read without
//! doctopus.
//!
//! A document becomes `NAME.md` in the output directory, and its pictures
//! and attached files go to `NAME.parts/`. Embedded documents are converted
//! too, to any depth: each gets a directory named after its id inside its
//! parent's parts, holding its own Markdown file and its own parts:
//!
//! ```text
//! report.md
//! report.parts/image1.png
//! report.parts/emb1/Budget.md
//! report.parts/emb1/image1.png
//! report.parts/emb1/emb2/Notes.md
//! ```
//!
//! Links are relative to the Markdown file that holds them, so the output
//! can be moved as a whole.
//!
//! Without `-o`, each document is converted next to its file. A Markdown
//! file already there is overwritten only when it is an earlier conversion
//! of a file with the same name; any other is left alone, and the
//! conversion takes the next free name (`report-2.md`).

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use doctopus_core::render::{self, RenderOptions, describe_part, estimate_tokens, shell_quote};
use doctopus_core::{Address, Document, Engine};
use serde::Serialize;

use crate::cli::{ConvertArgs, stem};
use crate::extract::{Skipped, empty_reason, unique_name};
use crate::failure::Failure;
use crate::load::{engine, load};
use crate::output::print;
use crate::paths;

/// What converting one document wrote.
#[derive(Debug, Serialize)]
struct Conversion {
    /// The converted document's address.
    source: String,
    /// The directory written to, absolute.
    directory: PathBuf,
    /// Markdown files: the document's own first, then those of embedded
    /// documents, depth first.
    markdown: Vec<Written>,
    /// Pictures and other files the Markdown links to.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    files: Vec<Written>,
    /// Parts with nothing to write.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skipped: Vec<Skipped>,
    /// Markdown files already in the directory, under the name this
    /// conversion would have used, that were not converted from this file.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    left_alone: Vec<PathBuf>,
    /// Warnings in all converted documents; each Markdown file lists its own
    /// in its YAML header.
    warnings: usize,
}

/// One written file.
#[derive(Debug, Serialize)]
struct Written {
    /// Where it was written, absolute.
    file: PathBuf,
    /// The address of the document or part it holds.
    address: String,
    /// Format id of the converted document, for Markdown files.
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'static str>,
    /// Media type, for other files.
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    /// Size in bytes.
    size: u64,
    /// Estimated tokens, for Markdown files.
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<usize>,
    /// One-line description.
    summary: String,
}

/// `doctopus convert`.
pub(crate) fn run(args: &ConvertArgs, max_depth: u32) -> Result<(), Failure> {
    let engine = engine(max_depth, args.formulas);
    let addresses: Vec<Address> = args.inputs.iter().map(|a| Address::parse(a)).collect();
    // Without -o, each file is converted next to itself.
    let dir = |address: &Address| {
        args.output
            .clone()
            .unwrap_or_else(|| paths::beside(address))
    };
    // Files taken in this run, so two inputs called `report` do not
    // overwrite each other.
    let mut taken = HashSet::new();
    if let [address] = addresses.as_slice() {
        let conversion = convert(&engine, address, &dir(address), &mut taken, args.formulas)?;
        return print(&if args.json {
            to_json(&conversion)
        } else {
            describe(&conversion)
        });
    }
    // Several files: one entry each, failures included, so converting a
    // directory is not cut short by one bad file.
    #[derive(Serialize)]
    #[serde(untagged)]
    enum Entry {
        Converted(Conversion),
        Failed { source: String, error: String },
    }
    let mut failed = None;
    let entries: Vec<Entry> = addresses
        .iter()
        .map(
            |address| match convert(&engine, address, &dir(address), &mut taken, args.formulas) {
                Ok(conversion) => Entry::Converted(conversion),
                Err(failure) => {
                    failed.get_or_insert(failure.code);
                    Entry::Failed {
                        source: address.to_string(),
                        error: failure.message,
                    }
                }
            },
        )
        .collect();
    let out = if args.json {
        to_json(&entries)
    } else {
        let texts: Vec<String> = entries
            .iter()
            .map(|entry| match entry {
                Entry::Converted(conversion) => describe(conversion),
                Entry::Failed { source, error } => format!("source: {source}\nerror: {error}\n"),
            })
            .collect();
        texts.join("---\n")
    };
    print(&out)?;
    match failed {
        Some(code) => Err(Failure::new(
            code,
            "some files could not be converted; see their `error` entries",
        )),
        None => Ok(()),
    }
}

fn convert(
    engine: &Engine,
    address: &Address,
    dir: &Path,
    taken: &mut HashSet<String>,
    formulas: bool,
) -> Result<Conversion, Failure> {
    let (document, _) = load(engine, address)?.into_document(address)?;
    let mut left_alone = Vec::new();
    let name = free_name(taken, dir, address, &mut left_alone);
    let mut converter = Converter {
        formulas,
        conversion: Conversion {
            source: address.to_string(),
            directory: paths::shown(dir),
            markdown: Vec::new(),
            files: Vec::new(),
            skipped: Vec::new(),
            left_alone: left_alone.iter().map(|f| paths::shown(f)).collect(),
            warnings: 0,
        },
    };
    converter.document(
        &document,
        address,
        &dir.join(format!("{name}.md")),
        &dir.join(format!("{name}.parts")),
        HashSet::new(),
    )?;
    let mut conversion = converter.conversion;
    for written in conversion.markdown.iter_mut().chain(&mut conversion.files) {
        written.file = paths::shown(&written.file);
    }
    Ok(conversion)
}

/// The document's stem, or `stem-2`, `stem-3`... when its Markdown file or
/// parts directory in `dir` is already taken in this run, or its Markdown
/// file is there and was not converted from this file. Such files are added
/// to `left_alone`.
fn free_name(
    taken: &mut HashSet<String>,
    dir: &Path,
    address: &Address,
    left_alone: &mut Vec<PathBuf>,
) -> String {
    let stem = stem(address);
    let mut n = 1;
    loop {
        let name = if n == 1 {
            stem.clone()
        } else {
            format!("{stem}-{n}")
        };
        let markdown = dir.join(format!("{name}.md"));
        // Names compare without case, as they do on Windows and macOS.
        let files = [markdown.clone(), dir.join(format!("{name}.parts"))]
            .map(|f| paths::shown(&f).to_string_lossy().to_lowercase());
        if files.iter().all(|f| !taken.contains(f)) {
            if may_overwrite(&markdown, address) {
                taken.extend(files);
                return name;
            }
            left_alone.push(markdown);
        }
        n += 1;
    }
}

/// Whether `markdown` may be written for `address`: it is not there, or it
/// is an earlier conversion of a file with the same name, as its YAML
/// header (`---\nsource: ...`) says.
fn may_overwrite(markdown: &Path, address: &Address) -> bool {
    let Ok(file) = std::fs::File::open(markdown) else {
        return !markdown.exists();
    };
    let mut head = Vec::new();
    if file.take(4096).read_to_end(&mut head).is_err() {
        return false;
    }
    let head = String::from_utf8_lossy(&head);
    let Some(source) = head
        .strip_prefix("---\nsource: ")
        .and_then(|rest| rest.lines().next())
    else {
        return false;
    };
    let source = Address::parse(source.trim().trim_matches(['"', '\'']));
    source.file.file_name() == address.file.file_name() && source.parts == address.parts
}

struct Converter {
    formulas: bool,
    conversion: Conversion,
}

impl Converter {
    /// Write `doc` as `markdown`, with its parts in `parts`. `used` holds
    /// the names already taken in `parts`.
    fn document(
        &mut self,
        doc: &Document,
        address: &Address,
        markdown: &Path,
        parts: &Path,
        mut used: HashSet<String>,
    ) -> Result<(), Failure> {
        let here = markdown.parent().unwrap_or(Path::new(""));
        // Embedded documents get a directory named after their id; keep
        // those names free for them.
        for part in doc.parts.iter().filter(|p| p.document.is_some()) {
            used.insert(part.id.as_str().to_lowercase());
        }
        let index = self.conversion.markdown.len();
        let mut links = BTreeMap::new();
        for part in &doc.parts {
            let part_address = address.child(part.id.as_str());
            if let Some(child) = &part.document {
                let dir = parts.join(part.id.as_str());
                let file = dir.join(markdown_name(&part.file_name()));
                let mut child_used = HashSet::new();
                child_used.extend(file.file_name().map(|n| n.to_string_lossy().to_lowercase()));
                self.document(child, &part_address, &file, &dir, child_used)?;
                links.insert(part.id.to_string(), paths::link(here, &file));
            } else if part.data.is_empty() {
                self.conversion.skipped.push(Skipped {
                    address: part_address.to_string(),
                    reason: empty_reason(part),
                });
            } else {
                let file = parts.join(unique_name(&mut used, part.file_name()));
                write(&file, &part.data)?;
                links.insert(part.id.to_string(), paths::link(here, &file));
                self.conversion.files.push(Written {
                    file,
                    address: part_address.to_string(),
                    format: None,
                    content_type: part.content_type.clone(),
                    size: part.data.len() as u64,
                    tokens: None,
                    summary: describe_part(part),
                });
            }
        }

        let mut options = RenderOptions::new(address.clone());
        options.standalone = true;
        options.formulas = self.formulas;
        options.extracted = links;
        let text = render::markdown(doc, &options).text;
        write(markdown, text.as_bytes())?;
        self.conversion.warnings += doc.warnings.len();
        let written = Written {
            file: markdown.to_path_buf(),
            address: address.to_string(),
            format: Some(doc.format.id),
            content_type: None,
            size: text.len() as u64,
            tokens: Some(estimate_tokens(&text)),
            summary: doc.format.name.to_string(),
        };
        // The parent before the documents embedded in it.
        self.conversion.markdown.insert(index, written);
        Ok(())
    }
}

/// The Markdown file name for a part saved as `file_name`: `Budget.md` for
/// `Budget.xlsx`.
fn markdown_name(file_name: &str) -> String {
    let stem = match file_name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file_name,
    };
    format!("{stem}.md")
}

fn write(file: &Path, data: &[u8]) -> Result<(), Failure> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|err| Failure::write(dir, &err))?;
    }
    std::fs::write(file, data).map_err(|err| Failure::write(file, &err))
}

/// The conversion as YAML-style text.
fn describe(conversion: &Conversion) -> String {
    let mut out = String::new();
    let dir = &conversion.directory;
    let quote = |path: &Path| shell_quote(&path.display().to_string());
    let listed = |file: &Path| quote(&paths::within(dir, file));
    let _ = writeln!(out, "source: {}", conversion.source);
    let _ = writeln!(
        out,
        "directory: {}  # the paths below are relative to it",
        quote(dir)
    );
    let _ = writeln!(out, "markdown: # {}", conversion.markdown.len());
    for written in &conversion.markdown {
        let _ = writeln!(
            out,
            "  - {}  # {}: {}, about {} tokens",
            listed(&written.file),
            written.address,
            written.summary,
            written.tokens.unwrap_or_default()
        );
    }
    if !conversion.files.is_empty() {
        let _ = writeln!(out, "files: # {}", conversion.files.len());
        for written in &conversion.files {
            let _ = writeln!(
                out,
                "  - {}  # {}: {}",
                listed(&written.file),
                written.address,
                written.summary
            );
        }
    }
    if !conversion.skipped.is_empty() {
        out.push_str("skipped:\n");
        for skipped in &conversion.skipped {
            let _ = writeln!(out, "  - {}: {}", skipped.address, skipped.reason);
        }
    }
    if !conversion.left_alone.is_empty() {
        out.push_str("left alone: # already there, and not converted from this file\n");
        for file in &conversion.left_alone {
            let _ = writeln!(out, "  - {}", listed(file));
        }
    }
    if conversion.warnings > 0 {
        let _ = writeln!(
            out,
            "warnings: {}  # listed in the YAML header of each Markdown file",
            conversion.warnings
        );
    }
    if let Some(first) = conversion.markdown.first() {
        let _ = writeln!(
            out,
            "next: read {}; it links to the Markdown of every embedded document and to the saved files",
            quote(&first.file)
        );
    }
    out
}

fn to_json(value: &impl Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_freed_per_run_and_other_files_are_left_alone() {
        let dir = std::env::temp_dir().join(format!("doctopus-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let report = Address::parse("in/report.docx");
        let mut left_alone = Vec::new();
        let mut taken = HashSet::new();
        assert_eq!(
            free_name(&mut taken, &dir, &report, &mut left_alone),
            "report"
        );
        let other = Address::parse("Report.xlsx");
        assert_eq!(
            free_name(&mut taken, &dir, &other, &mut left_alone),
            "Report-2"
        );
        assert!(left_alone.is_empty());

        // In a later run, the user's own report.md is kept, and an earlier
        // conversion of report.docx is overwritten.
        std::fs::write(dir.join("report.md"), "# My notes\n").unwrap();
        let earlier = "---\nsource: elsewhere/report.docx\nformat: docx\n---\n";
        std::fs::write(dir.join("report-2.md"), earlier).unwrap();
        let name = free_name(&mut HashSet::new(), &dir, &report, &mut left_alone);
        assert_eq!(name, "report-2");
        assert_eq!(left_alone, [dir.join("report.md")]);
        // A conversion of another file called `report` is kept too.
        left_alone.clear();
        let pdf = Address::parse("report.pdf");
        let name = free_name(&mut HashSet::new(), &dir, &pdf, &mut left_alone);
        assert_eq!(name, "report-3");
        assert_eq!(left_alone, [dir.join("report.md"), dir.join("report-2.md")]);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(markdown_name("Budget.xlsx"), "Budget.md");
        assert_eq!(markdown_name("README"), "README.md");
        assert_eq!(markdown_name(".hidden"), ".hidden.md");
    }
}
