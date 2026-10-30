//! A compact overview of a document: what it is, how it is organised, what
//! it carries, and how big it is when rendered.

use std::fmt::Write as _;

use serde::Serialize;

use super::markdown::describe_part;
use super::{RenderOptions, estimate_tokens, human_size, shell_quote, yaml};
use crate::{
    Address, Block, Document, FormatInfo, Metadata, Part, Section, SectionKind, plain_text,
};

/// Outline entries listed before eliding the rest.
const MAX_OUTLINE: usize = 150;
/// Sections listed before eliding the rest.
const MAX_SECTIONS: usize = 200;
/// Parts listed before eliding the rest.
const MAX_PARTS: usize = 300;

/// Structured overview of a document, as produced by [`summarize`].
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    /// The address that was summarised.
    pub source: String,
    /// Detected format.
    pub format: &'static FormatInfo,
    /// Size of the input in bytes, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Document properties.
    #[serde(skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// One entry per section.
    pub sections: Vec<SectionSummary>,
    /// Headings of word-processing documents.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<OutlineEntry>,
    /// Every part, nested ones included, depth first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartSummary>,
    /// Size of the full Markdown rendering.
    pub rendered_chars: usize,
    /// Estimated tokens of the full Markdown rendering.
    pub rendered_tokens: usize,
    /// Problems met while reading.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Overview of one section.
#[derive(Debug, Clone, Serialize)]
pub struct SectionSummary {
    /// Section kind.
    pub kind: SectionKind,
    /// Section number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// Section name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether the section is hidden.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// What the section contains, in a few words.
    pub contents: String,
}

/// A heading.
#[derive(Debug, Clone, Serialize)]
pub struct OutlineEntry {
    /// Heading level, 1 for the top.
    pub level: u8,
    /// Heading text.
    pub text: String,
}

/// Overview of one part.
#[derive(Debug, Clone, Serialize)]
pub struct PartSummary {
    /// Path of ids from the summarised document (`emb1/img2`).
    pub path: String,
    /// Full address, usable as a command argument.
    pub address: String,
    /// Nesting depth (0 for direct parts).
    pub depth: usize,
    /// The part itself (payload excluded).
    #[serde(flatten)]
    pub part: PartInfo,
}

/// Part fields worth reporting, without payload or nested document.
#[derive(Debug, Clone, Serialize)]
pub struct PartInfo {
    /// Kind of part.
    pub kind: crate::PartKind,
    /// File name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Media type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Format id of the parsed content, when doctopus can read it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<&'static str>,
    /// Size in bytes.
    pub size: u64,
    /// Pixel size of images.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<(u32, u32)>,
    /// Where the part appears.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Why the content could not be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// One-line description used in text output.
    pub summary: String,
}

/// Build the overview of `doc`. `size` is the input size in bytes, if known.
pub fn summarize(doc: &Document, options: &RenderOptions, size: Option<u64>) -> Summary {
    let mut full = options.clone();
    full.front_matter = true;
    let rendered = super::markdown(doc, &full).text;
    let mut parts = Vec::new();
    collect_parts(doc, &options.source, &mut Vec::new(), &mut parts);
    Summary {
        source: options.source.to_string(),
        format: doc.format,
        size,
        metadata: doc.metadata.clone(),
        sections: doc.sections.iter().map(summarize_section).collect(),
        outline: outline(doc),
        parts,
        rendered_chars: rendered.chars().count(),
        rendered_tokens: estimate_tokens(&rendered),
        warnings: doc.warnings.clone(),
    }
}

/// Render the overview as YAML-style text, ending with suggested commands.
pub fn info(doc: &Document, options: &RenderOptions, size: Option<u64>) -> String {
    let s = summarize(doc, options, size);
    let mut out = String::new();
    let source = options.source.to_string();
    let _ = writeln!(out, "source: {}", yaml::scalar(&source));
    let _ = writeln!(
        out,
        "format: {}",
        yaml::scalar(&format!("{} ({})", s.format.id, s.format.name))
    );
    if let Some(size) = s.size {
        let _ = writeln!(out, "size: {}", human_size(size));
    }
    for (k, v) in s.metadata.fields() {
        let value = v.split_whitespace().collect::<Vec<_>>().join(" ");
        let _ = writeln!(out, "{}: {}", yaml::key(k), yaml::scalar(&value));
    }

    let primary: Vec<&SectionSummary> = s.sections.iter().filter(|x| x.kind.is_primary()).collect();
    if let Some(first) = primary.first() {
        let noun = match first.kind {
            SectionKind::Sheet => "sheets",
            SectionKind::Slide => "slides",
            _ => "pages",
        };
        let _ = writeln!(out, "{noun}: # {}", primary.len());
        for section in primary.iter().take(MAX_SECTIONS) {
            let mut label = section.number.map(|n| n.to_string()).unwrap_or_default();
            if let Some(name) = &section.name {
                let _ = write!(label, " \"{name}\"");
            }
            if section.hidden {
                label.push_str(" (hidden)");
            }
            let _ = writeln!(
                out,
                "  - {}",
                yaml::scalar(&format!("{}: {}", label.trim(), section.contents))
            );
        }
        if primary.len() > MAX_SECTIONS {
            let _ = writeln!(out, "  - \"... {} more\"", primary.len() - MAX_SECTIONS);
        }
    }
    let others: Vec<&SectionSummary> = s.sections.iter().filter(|x| !x.kind.is_primary()).collect();
    if !others.is_empty() {
        out.push_str("sections:\n");
        for section in others {
            let name = section
                .name
                .clone()
                .unwrap_or_else(|| section.kind.noun().to_string());
            let _ = writeln!(
                out,
                "  - {}",
                yaml::scalar(&format!("{name}: {}", section.contents))
            );
        }
    }
    if !s.outline.is_empty() {
        out.push_str("outline:\n");
        for entry in s.outline.iter().take(MAX_OUTLINE) {
            let indent = "  ".repeat(usize::from(entry.level.saturating_sub(1).min(5)));
            let _ = writeln!(out, "  {indent}- {}", yaml::scalar(&entry.text));
        }
        if s.outline.len() > MAX_OUTLINE {
            let _ = writeln!(
                out,
                "  - \"... {} more headings\"",
                s.outline.len() - MAX_OUTLINE
            );
        }
    }
    if !s.parts.is_empty() {
        let _ = writeln!(out, "parts: # {}", s.parts.len());
        for p in s.parts.iter().take(MAX_PARTS) {
            let indent = "  ".repeat(p.depth);
            let _ = writeln!(
                out,
                "  {indent}- {}: {}",
                p.path,
                yaml::scalar(&p.part.summary)
            );
        }
        if s.parts.len() > MAX_PARTS {
            let _ = writeln!(out, "  - \"... {} more\"", s.parts.len() - MAX_PARTS);
        }
    }
    let _ = writeln!(
        out,
        "rendered: {}",
        yaml::scalar(&format!(
            "~{} characters (~{} tokens) as Markdown",
            group_digits(s.rendered_chars),
            group_digits(s.rendered_tokens)
        ))
    );
    if !s.warnings.is_empty() {
        out.push_str("warnings:\n");
        for w in &s.warnings {
            let _ = writeln!(out, "  - {}", yaml::scalar(w));
        }
    }
    out.push_str("next:\n");
    for (command, why) in suggestions(doc, &options.source, &s) {
        let _ = writeln!(out, "  - {command}  # {why}");
    }
    out
}

fn suggestions(doc: &Document, source: &Address, s: &Summary) -> Vec<(String, String)> {
    let q = shell_quote(&source.to_string());
    let mut next = vec![(
        format!("doctopus read {q}"),
        "full content as Markdown".to_string(),
    )];
    if s.rendered_chars > 25_000 {
        if let Some(first) = s.sections.iter().find(|x| x.kind.is_primary()) {
            let flag = match first.kind {
                SectionKind::Sheet => "--sheet",
                SectionKind::Slide => "--slide",
                _ => "--page",
            };
            let example = first.number.unwrap_or(1);
            next.push((
                format!("doctopus read {q} {flag} {example}"),
                format!("one {} at a time", first.kind.noun()),
            ));
        }
        next.push((
            format!("doctopus read {q} -o out.md"),
            "write everything to a file and read it in chunks".to_string(),
        ));
    }
    if let Some(first_doc) = s.parts.iter().find(|p| p.part.format.is_some()) {
        next.push((
            format!("doctopus read {}", shell_quote(&first_doc.address)),
            format!(
                "only the embedded {}",
                first_doc.part.format.unwrap_or("document")
            ),
        ));
    }
    if !doc.parts.is_empty() {
        next.push((
            format!("doctopus extract {q}"),
            "save images and embedded files next to the file".to_string(),
        ));
    }
    next
}

fn collect_parts(
    doc: &Document,
    root: &Address,
    path: &mut Vec<String>,
    out: &mut Vec<PartSummary>,
) {
    for part in &doc.parts {
        path.push(part.id.to_string());
        let mut address = root.clone();
        address.parts.extend(path.iter().cloned());
        out.push(PartSummary {
            path: path.join("/"),
            address: address.to_string(),
            depth: path.len() - 1,
            part: part_info(part),
        });
        if let Some(child) = &part.document {
            collect_parts(child, root, path, out);
        }
        path.pop();
    }
}

fn part_info(part: &Part) -> PartInfo {
    PartInfo {
        kind: part.kind,
        name: part.name.clone(),
        content_type: part.content_type.clone(),
        format: part.document.as_ref().map(|d| d.format.id),
        size: part.size,
        dimensions: part.dimensions,
        location: part.location.clone(),
        description: part.description.clone(),
        error: part.error.clone(),
        summary: describe_part(part),
    }
}

fn summarize_section(section: &Section) -> SectionSummary {
    let mut stats = Stats::default();
    stats.add_blocks(&section.blocks);
    let mut facts = Vec::new();
    if let Some(range) = section.properties.get("range") {
        facts.push(range.clone());
    }
    if stats.cells > 0 {
        facts.push(plural(stats.cells, "cell"));
    }
    if stats.words > 0 {
        facts.push(plural(stats.words, "word"));
    }
    for (n, what) in [
        (stats.tables, "table"),
        (stats.images, "image"),
        (stats.charts, "chart"),
        (stats.embeds, "embedded object"),
        (stats.shapes, "shape"),
        (stats.connections, "connection"),
        (stats.notes, "note"),
    ] {
        if n > 0 {
            facts.push(plural(n, what));
        }
    }
    if stats.speaker_notes {
        facts.push("speaker notes".into());
    }
    SectionSummary {
        kind: section.kind,
        number: section.number,
        name: section.name.clone(),
        hidden: section.hidden,
        contents: if facts.is_empty() {
            "empty".into()
        } else {
            facts.join(", ")
        },
    }
}

fn outline(doc: &Document) -> Vec<OutlineEntry> {
    let mut entries = Vec::new();
    for section in doc.sections.iter().filter(|s| s.kind == SectionKind::Body) {
        for block in &section.blocks {
            if let Block::Heading(h) = block {
                let text = plain_text(&h.content)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if !text.is_empty() {
                    entries.push(OutlineEntry {
                        level: h.level,
                        text,
                    });
                }
            }
        }
    }
    entries
}

#[derive(Default)]
struct Stats {
    words: usize,
    cells: usize,
    tables: usize,
    images: usize,
    charts: usize,
    embeds: usize,
    shapes: usize,
    connections: usize,
    notes: usize,
    speaker_notes: bool,
}

impl Stats {
    fn add_blocks(&mut self, blocks: &[Block]) {
        for block in blocks {
            match block {
                Block::Heading(h) => self.add_inlines(&h.content),
                Block::Paragraph(p) => self.add_inlines(&p.content),
                Block::ListItem(l) => self.add_inlines(&l.content),
                Block::Table(t) => {
                    self.tables += 1;
                    for row in &t.rows {
                        for cell in &row.cells {
                            self.add_blocks(&cell.blocks);
                        }
                    }
                }
                Block::Grid(g) => self.cells += g.cell_count(),
                Block::Image(_) => self.images += 1,
                Block::Embed(_) => self.embeds += 1,
                Block::Chart(_) => self.charts += 1,
                Block::Diagram(d) => {
                    self.shapes += count_shapes(&d.shapes);
                    self.connections += d.connections.len();
                }
                Block::Math(_) | Block::Code(_) => {}
                Block::Note(n) => {
                    self.notes += 1;
                    self.add_blocks(&n.blocks);
                }
                Block::Group(g) => {
                    if g.kind == crate::GroupKind::Notes {
                        self.speaker_notes = true;
                    } else {
                        self.add_blocks(&g.blocks);
                    }
                }
            }
        }
    }

    fn add_inlines(&mut self, content: &[crate::Inline]) {
        self.words += plain_text(content).split_whitespace().count();
        self.images += content
            .iter()
            .filter(|i| matches!(i, crate::Inline::Image(_)))
            .count();
    }
}

fn count_shapes(shapes: &[crate::Shape]) -> usize {
    shapes.iter().map(|s| 1 + count_shapes(&s.children)).sum()
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{} {what}s", group_digits(n))
    }
}

fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
