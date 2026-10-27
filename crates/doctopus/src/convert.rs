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

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
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
    /// Markdown files: the document's own first, then those of embedded
    /// documents, depth first.
    markdown: Vec<Written>,
    /// Pictures and other files the Markdown links to.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    files: Vec<Written>,
    /// Parts with nothing to write.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skipped: Vec<Skipped>,
    /// Warnings in all converted documents; each Markdown file lists its own
    /// in its YAML header.
    warnings: usize,
}

/// One written file.
#[derive(Debug, Serialize)]
struct Written {
    /// Where it was written.
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
    let dir = args.output.clone().unwrap_or_default();
    let addresses: Vec<Address> = args.inputs.iter().map(|a| Address::parse(a)).collect();
    // Names taken in the output directory, so two inputs called `report`
    // do not overwrite each other.
    let mut taken = HashSet::new();
    if let [address] = addresses.as_slice() {
        let conversion = convert(&engine, address, &dir, &mut taken, args.formulas)?;
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
            |address| match convert(&engine, address, &dir, &mut taken, args.formulas) {
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
    let name = free_name(taken, &stem(address));
    let mut converter = Converter {
        formulas,
        conversion: Conversion {
            source: address.to_string(),
            markdown: Vec::new(),
            files: Vec::new(),
            skipped: Vec::new(),
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
    Ok(converter.conversion)
}

/// `stem`, or `stem-2`, `stem-3`... when its Markdown file or parts
/// directory is already taken in this run.
fn free_name(taken: &mut HashSet<String>, stem: &str) -> String {
    let mut n = 1;
    loop {
        let name = if n == 1 {
            stem.to_string()
        } else {
            format!("{stem}-{n}")
        };
        let files = [format!("{name}.md"), format!("{name}.parts")].map(|f| f.to_lowercase());
        if files.iter().all(|f| !taken.contains(f)) {
            taken.extend(files);
            return name;
        }
        n += 1;
    }
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
    let quote = |path: &Path| shell_quote(&path.display().to_string());
    let _ = writeln!(out, "source: {}", conversion.source);
    let _ = writeln!(out, "markdown: # {}", conversion.markdown.len());
    for written in &conversion.markdown {
        let _ = writeln!(
            out,
            "  - {}  # {}: {}, about {} tokens",
            quote(&written.file),
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
                quote(&written.file),
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
    fn names_are_freed_per_run_and_markdown_names_keep_the_stem() {
        let mut taken = HashSet::new();
        assert_eq!(free_name(&mut taken, "report"), "report");
        assert_eq!(free_name(&mut taken, "Report"), "Report-2");
        assert_eq!(markdown_name("Budget.xlsx"), "Budget.md");
        assert_eq!(markdown_name("README"), "README.md");
        assert_eq!(markdown_name(".hidden"), ".hidden.md");
    }
}
