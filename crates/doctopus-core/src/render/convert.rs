//! Documents as Markdown files that are read without doctopus, as
//! `doctopus convert` writes them.
//!
//! A document becomes `NAME.md`, and its pictures and attached files go to
//! `NAME.parts/`. Embedded documents are converted too, to any depth: each
//! gets a directory named after its id inside its parent's parts, holding
//! its own Markdown file and its own parts:
//!
//! ```text
//! report.md
//! report.parts/image1.png
//! report.parts/emb1/Budget.md
//! report.parts/emb1/image1.png
//! report.parts/emb1/emb2/Notes.md
//! ```
//!
//! Links are relative to the Markdown file that holds them, so the files
//! can be moved as a whole. [`convert`] builds them in memory; writing them
//! somewhere is up to the caller.

use std::collections::{BTreeMap, HashSet};

use super::{RenderOptions, estimate_tokens};
use crate::{Address, Document, Part};

/// A document converted to linked files, not written anywhere yet.
#[derive(Debug)]
pub struct Converted<'a> {
    /// Markdown files: the document's own first, then those of embedded
    /// documents, depth first.
    pub markdown: Vec<MarkdownFile<'a>>,
    /// Pictures and other files the Markdown links to.
    pub files: Vec<PartFile<'a>>,
    /// Parts with nothing to write.
    pub skipped: Vec<SkippedPart>,
    /// Warnings in all converted documents; each Markdown file lists its own
    /// in its YAML header.
    pub warnings: usize,
}

/// The Markdown file of a document.
#[derive(Debug)]
pub struct MarkdownFile<'a> {
    /// Where it goes, relative to the output directory, with `/` between
    /// components: `report.md`, `report.parts/emb1/Budget.md`.
    pub path: String,
    /// The address of the document.
    pub address: Address,
    /// The document.
    pub document: &'a Document,
    /// Its Markdown.
    pub text: String,
}

impl MarkdownFile<'_> {
    /// Estimated tokens of the Markdown.
    pub fn tokens(&self) -> usize {
        estimate_tokens(&self.text)
    }
}

/// A part saved as it is: a picture, an attachment, an embedded file
/// doctopus cannot read.
#[derive(Debug)]
pub struct PartFile<'a> {
    /// Where it goes, relative to the output directory, with `/` between
    /// components: `report.parts/image1.png`.
    pub path: String,
    /// The address of the part.
    pub address: Address,
    /// The part; its payload is the file's content.
    pub part: &'a Part,
}

/// A part with nothing to write.
#[derive(Debug)]
pub struct SkippedPart {
    /// The address of the part.
    pub address: Address,
    /// Why there is nothing to write.
    pub reason: String,
}

/// Convert `doc` to `NAME.md` and `NAME.parts/`, with `name` as `NAME`.
///
/// `options.source` is the document's address; the other options apply to
/// every Markdown file, except that [`standalone`](RenderOptions::standalone)
/// is always on and [`extracted`](RenderOptions::extracted) is filled in
/// with the links between the files.
pub fn convert<'a>(doc: &'a Document, name: &str, options: &RenderOptions) -> Converted<'a> {
    let mut converter = Converter {
        options,
        converted: Converted {
            markdown: Vec::new(),
            files: Vec::new(),
            skipped: Vec::new(),
            warnings: 0,
        },
    };
    converter.document(
        doc,
        &options.source,
        format!("{name}.md"),
        &format!("{name}.parts"),
        HashSet::new(),
    );
    converter.converted
}

struct Converter<'o, 'a> {
    options: &'o RenderOptions,
    converted: Converted<'a>,
}

impl<'a> Converter<'_, 'a> {
    /// Convert `doc` to the Markdown file `markdown`, with its parts in the
    /// directory `parts`. `used` holds the names already taken in `parts`.
    fn document(
        &mut self,
        doc: &'a Document,
        address: &Address,
        markdown: String,
        parts: &str,
        mut used: HashSet<String>,
    ) {
        let here = markdown.rsplit_once('/').map_or("", |(dir, _)| dir);
        // Embedded documents get a directory named after their id; keep
        // those names free for them.
        for part in doc.parts.iter().filter(|p| p.document.is_some()) {
            used.insert(part.id.as_str().to_lowercase());
        }
        let index = self.converted.markdown.len();
        let mut links = BTreeMap::new();
        for part in &doc.parts {
            let part_address = address.child(part.id.as_str());
            if let Some(child) = &part.document {
                let dir = join(parts, part.id.as_str());
                let name = markdown_name(&part.file_name());
                let file = join(&dir, &name);
                links.insert(part.id.to_string(), link(here, &file));
                let child_used = HashSet::from([name.to_lowercase()]);
                self.document(child, &part_address, file, &dir, child_used);
            } else if part.data.is_empty() {
                self.converted.skipped.push(SkippedPart {
                    address: part_address,
                    reason: empty_reason(part),
                });
            } else {
                let file = join(parts, &unique_name(&mut used, part.file_name()));
                links.insert(part.id.to_string(), link(here, &file));
                self.converted.files.push(PartFile {
                    path: file,
                    address: part_address,
                    part,
                });
            }
        }

        let mut options = self.options.clone();
        options.source = address.clone();
        options.standalone = true;
        options.extracted = links;
        let text = super::markdown(doc, &options).text;
        self.converted.warnings += doc.warnings.len();
        // The parent before the documents embedded in it.
        self.converted.markdown.insert(
            index,
            MarkdownFile {
                path: markdown,
                address: address.clone(),
                document: doc,
                text,
            },
        );
    }
}

/// Why a part without data has nothing to save.
pub fn empty_reason(part: &Part) -> String {
    match &part.link {
        Some(link) => format!("linked, not embedded: {link}"),
        None => "no content".into(),
    }
}

/// `name`, or `name-2`, `name-3`... if it is taken. Names compare without
/// case, as they do on Windows and macOS file systems.
pub fn unique_name(used: &mut HashSet<String>, name: String) -> String {
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

/// The Markdown file name for a part saved as `file_name`: `Budget.md` for
/// `Budget.xlsx`.
fn markdown_name(file_name: &str) -> String {
    let stem = match file_name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file_name,
    };
    format!("{stem}.md")
}

/// `name` in the directory `dir`, both relative to the output directory.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// `file` as a link from a Markdown file in the directory `from`, both
/// relative to the output directory.
fn link(from: &str, file: &str) -> String {
    let from: Vec<&str> = from.split('/').filter(|c| !c.is_empty()).collect();
    let file: Vec<&str> = file.split('/').collect();
    let common = from.iter().zip(&file).take_while(|(a, b)| a == b).count();
    let mut out = vec![".."; from.len() - common];
    out.extend(&file[common..]);
    out.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DocumentKind, FormatInfo, PartKind, Section, SectionKind};

    static TEST_FORMAT: FormatInfo = FormatInfo {
        id: "test",
        name: "Test document",
        kind: DocumentKind::Text,
        extensions: &["test"],
        mime: "application/x-test",
    };

    #[test]
    fn embedded_documents_get_their_own_directories() {
        let mut notes = Document::new(&TEST_FORMAT);
        notes.sections.push(Section::new(SectionKind::Body));
        notes.add_part(Part::new(PartKind::Image, vec![1]).with_name("image1.png"));
        let mut budget = Document::new(&TEST_FORMAT);
        budget.warn("one");
        budget.add_part(Part::new(PartKind::Image, vec![2]).with_name("image1.png"));
        let mut embedded = Part::new(PartKind::Object, vec![0; 4]).with_name("Notes (draft).doc");
        embedded.document = Some(Box::new(notes));
        budget.add_part(embedded);

        let mut doc = Document::new(&TEST_FORMAT);
        doc.add_part(Part::new(PartKind::Image, vec![3]).with_name("image1.png"));
        let mut embedded = Part::new(PartKind::Object, vec![0; 4]).with_name("Budget.xlsx");
        embedded.document = Some(Box::new(budget));
        doc.add_part(embedded);
        // A picture named like an embedded document's directory moves aside.
        doc.add_part(Part::new(PartKind::Image, vec![4]).with_name("EMB1"));
        doc.add_part(Part::new(PartKind::Image, vec![5]).with_name("Image1.png"));
        doc.add_part(Part::new(PartKind::Image, Vec::new()).with_link("https://example.com/a.png"));

        let options = RenderOptions::new(Address::parse_with("in/report.test", |_| true));
        let converted = convert(&doc, "report", &options);
        let markdown: Vec<_> = converted.markdown.iter().map(|m| m.path.as_str()).collect();
        assert_eq!(
            markdown,
            [
                "report.md",
                "report.parts/emb1/Budget.md",
                "report.parts/emb1/emb1/Notes (draft).md"
            ]
        );
        let files: Vec<_> = converted
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.address.to_string()))
            .collect();
        assert_eq!(
            files,
            [
                ("report.parts/image1.png", "in/report.test#img1".into()),
                (
                    "report.parts/emb1/image1.png",
                    "in/report.test#emb1/img1".into()
                ),
                (
                    "report.parts/emb1/emb1/image1.png",
                    "in/report.test#emb1/emb1/img1".into()
                ),
                ("report.parts/EMB1-2", "in/report.test#img2".into()),
                ("report.parts/Image1-2.png", "in/report.test#img3".into()),
            ]
        );
        assert_eq!(converted.skipped.len(), 1);
        assert_eq!(
            converted.skipped[0].reason,
            "linked, not embedded: https://example.com/a.png"
        );
        assert_eq!(converted.warnings, 1);

        // Each file links to its own parts, relative to itself.
        let budget = &converted.markdown[1];
        assert_eq!(budget.address.to_string(), "in/report.test#emb1");
        assert!(budget.text.contains("](image1.png"), "{}", budget.text);
        assert!(
            budget.text.contains("](<emb1/Notes (draft).md>"),
            "{}",
            budget.text
        );
    }

    #[test]
    fn links_are_relative_to_the_markdown_file() {
        assert_eq!(link("", "report.parts/a.png"), "report.parts/a.png");
        assert_eq!(
            link("report.parts/emb1", "report.parts/emb1/a.png"),
            "a.png"
        );
        assert_eq!(link("report.parts/emb1", "report.parts/a.png"), "../a.png");
        assert_eq!(link("a/b", "c/d.png"), "../../c/d.png");
        assert_eq!(markdown_name("Budget.xlsx"), "Budget.md");
        assert_eq!(markdown_name("README"), "README.md");
        assert_eq!(markdown_name(".hidden"), ".hidden.md");
    }
}
