//! Turning documents into text for people and agents.
//!
//! * [`markdown`] — the default: structure-preserving Markdown with a YAML
//!   header, stable part references and embedded documents inlined between
//!   `<embedded>` tags.
//! * [`text`] — the same content without Markdown syntax.
//! * [`json`] — the full document model, for programs.
//! * [`info`] — a compact overview: format, metadata, structure, parts.
//! * [`convert`] — linked Markdown files read without doctopus: one per
//!   document, embedded ones included, plus the pictures and files they
//!   link to.
//!
//! Output is deterministic, so it can be paged with [`paginate`] and resumed
//! from any offset.

mod convert;
mod info;
mod json;
mod markdown;
mod yaml;

use std::collections::BTreeMap;

pub use self::convert::{
    Converted, MarkdownFile, PartFile, SkippedPart, convert, empty_reason, unique_name,
};
pub use self::info::{
    OutlineEntry, PartInfo, PartSummary, SectionSummary, Summary, info, summarize,
};
pub use self::json::json;
pub use self::markdown::{describe_part, markdown, text};
use crate::{Address, SectionKind};

/// Options shared by the renderers.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RenderOptions {
    /// How the rendered document is addressed by the caller (`report.docx`,
    /// `report.docx#emb1`). Part references are built from it.
    pub source: Address,
    /// How many levels of embedded documents to render inline. Deeper
    /// documents are referenced instead.
    pub embed_depth: u32,
    /// Show spreadsheet formulas next to their values.
    pub formulas: bool,
    /// Start Markdown output with a YAML header.
    pub front_matter: bool,
    /// Files that parts were extracted to, keyed by part path (`emb1/img2`).
    /// Rendered references point at these files instead of addresses.
    pub extracted: BTreeMap<String, String>,
    /// Write Markdown that is read without doctopus, as `doctopus convert`
    /// does: embedded documents are not inlined but linked as the files in
    /// [`extracted`](Self::extracted) (their own conversions), output never
    /// suggests doctopus commands, and diagrams with connections also get a
    /// Mermaid flowchart.
    pub standalone: bool,
}

impl RenderOptions {
    /// Defaults for a document addressed as `source`.
    pub fn new(source: Address) -> Self {
        Self {
            source,
            embed_depth: 8,
            formulas: false,
            front_matter: true,
            extracted: BTreeMap::new(),
            standalone: false,
        }
    }
}

/// Rendered output plus the position where each top-level section starts,
/// so callers can describe what a page of output covers.
#[derive(Debug, Clone)]
pub struct Rendered {
    /// The output.
    pub text: String,
    /// Section start positions, in characters, in output order.
    pub marks: Vec<SectionMark>,
}

/// Where a top-level section starts in [`Rendered::text`].
#[derive(Debug, Clone)]
pub struct SectionMark {
    /// Section kind.
    pub kind: SectionKind,
    /// Section number, if any.
    pub number: Option<u32>,
    /// Section name, if any.
    pub name: Option<String>,
    /// Character offset of the section heading.
    pub offset: usize,
}

impl SectionMark {
    /// A short label such as `slide 3` or `sheet 2 "Data"`.
    pub fn label(&self) -> String {
        let mut label = self.kind.noun().to_string();
        if let Some(n) = self.number {
            label.push_str(&format!(" {n}"));
        }
        if let Some(name) = &self.name {
            label.push_str(&format!(" \"{name}\""));
        }
        label
    }
}

/// One window of a longer output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page<'a> {
    /// The text of this page.
    pub text: &'a str,
    /// Character offset where the page starts.
    pub start: usize,
    /// Character offset just past the end of the page.
    pub end: usize,
    /// Total characters in the whole output.
    pub total: usize,
}

impl Page<'_> {
    /// Whether there is more output after this page.
    pub fn is_truncated(&self) -> bool {
        self.end < self.total
    }
}

/// Cut `text` into a page of at most `max_chars` characters starting at
/// character `offset` (`max_chars == 0` means unlimited).
///
/// Pages end at a line break whenever one exists in the second half of the
/// window, so a page boundary never splits a line, table row or word in
/// half if it can be avoided; the returned [`Page::end`] is exactly where the
/// next page should start.
pub fn paginate(text: &str, offset: usize, max_chars: usize) -> Page<'_> {
    let total = text.chars().count();
    let start = offset.min(total);
    let start_byte = char_to_byte(text, start);
    let rest = &text[start_byte..];
    if max_chars == 0 || total - start <= max_chars {
        return Page {
            text: rest,
            start,
            end: total,
            total,
        };
    }
    let hard_end = char_to_byte(rest, max_chars);
    let window = &rest[..hard_end];
    let cut = match window.rfind('\n') {
        Some(nl) if nl + 1 >= hard_end / 2 => nl + 1,
        _ => hard_end,
    };
    let page = &rest[..cut];
    Page {
        text: page,
        start,
        end: start + page.chars().count(),
        total,
    }
}

fn char_to_byte(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i)
}

/// A rough token count for budgeting: about four characters per token for
/// ASCII text, one token per character for other scripts.
pub fn estimate_tokens(text: &str) -> usize {
    let (ascii, other) = text.chars().fold((0usize, 0usize), |(a, o), c| {
        if c.is_ascii() { (a + 1, o) } else { (a, o + 1) }
    });
    ascii.div_ceil(4) + other
}

/// A byte count for people: `812 B`, `12.4 KB`, `3.1 MB`.
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / KB / KB)
    } else {
        format!("{:.1} GB", b / KB / KB / KB)
    }
}

/// Quote an address for use in a shell command shown to the caller.
///
/// Addresses containing `#` are always quoted: in zsh with `extendedglob`
/// an unquoted `#` is a glob operator. Double quotes work in POSIX shells,
/// PowerShell and cmd alike.
pub fn shell_quote(arg: &str) -> String {
    let plain = arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
    if plain && !arg.is_empty() {
        arg.to_string()
    } else {
        format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_end_on_line_boundaries_and_resume_exactly() {
        let text = "line one\nline two\nline three\n";
        let first = paginate(text, 0, 15);
        assert_eq!(first.text, "line one\n");
        assert!(first.is_truncated());
        let second = paginate(text, first.end, 15);
        assert_eq!(second.text, "line two\n");
        let rest = paginate(text, second.end, 0);
        assert_eq!(rest.text, "line three\n");
        assert!(!rest.is_truncated());
    }

    #[test]
    fn pagination_counts_characters_not_bytes() {
        let text = "ééé\nèèè\n";
        let page = paginate(text, 0, 5);
        assert_eq!(page.text, "ééé\n");
        assert_eq!(page.end, 4);
        assert_eq!(page.total, 8);
    }

    #[test]
    fn quotes_shell_arguments() {
        assert_eq!(shell_quote("report.docx"), "report.docx");
        assert_eq!(shell_quote("report.docx#emb1"), "\"report.docx#emb1\"");
        assert_eq!(shell_quote("My \"Q3\".xlsx"), "\"My \\\"Q3\\\".xlsx\"");
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(812), "812 B");
        assert_eq!(human_size(12_700), "12.4 KB");
        assert_eq!(human_size(3_250_000), "3.1 MB");
    }
}
