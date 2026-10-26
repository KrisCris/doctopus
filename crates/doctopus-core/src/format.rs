//! The plug-in contract: every supported file type is a [`Reader`].
//!
//! A reader declares the formats it understands ([`FormatInfo`]), recognises
//! them from a cheap [`Probe`], and turns raw bytes into a [`Document`]. Readers
//! never parse embedded content themselves: they attach it as a
//! [`Part`](crate::Part) and the [`Engine`](crate::Engine) recursively hands it
//! to whichever reader recognises it. Adding a file type therefore means
//! implementing this trait and registering the reader; nothing else changes.

use std::fmt;

use serde::Serialize;

use crate::{Document, Probe, ReadOptions, Result};

/// Static description of one file format.
#[derive(Debug, Serialize)]
pub struct FormatInfo {
    /// Short stable identifier, conventionally the main file extension
    /// (`"docx"`, `"xls"`, `"vsd"`).
    pub id: &'static str,
    /// Human-readable name, e.g. `"Word document (Office Open XML)"`.
    pub name: &'static str,
    /// What kind of content the format holds.
    pub kind: DocumentKind,
    /// Usual file extensions, without the dot. Used only as a hint.
    #[serde(skip)]
    pub extensions: &'static [&'static str],
    /// Canonical media type.
    pub mime: &'static str,
}

impl fmt::Display for FormatInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.id, self.name)
    }
}

/// Broad category of a document. Decides how sections are labelled
/// (sheets, slides, pages) when rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    /// Word processing: a flow of paragraphs, tables and figures.
    Text,
    /// Workbooks made of sheets of cells.
    Spreadsheet,
    /// Slide decks.
    Presentation,
    /// Drawings made of shapes and connectors, organised in pages.
    Diagram,
    /// Containers of other files (zip archives, packages).
    Archive,
    /// Anything else.
    Other,
}

/// How sure a reader is that it recognises the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Plausible, for example only the file extension matches.
    Low,
    /// The container matches but the signature is not conclusive.
    Medium,
    /// A conclusive signature was found.
    High,
}

/// A successful [`Reader::detect`] result.
#[derive(Debug, Clone, Copy)]
pub struct Detection {
    /// Which of the reader's formats was recognised.
    pub format: &'static FormatInfo,
    /// How certain the match is; the registry picks the highest.
    pub confidence: Confidence,
}

impl Detection {
    /// A conclusive match.
    pub fn certain(format: &'static FormatInfo) -> Self {
        Self {
            format,
            confidence: Confidence::High,
        }
    }

    /// A match with the given confidence.
    pub fn new(format: &'static FormatInfo, confidence: Confidence) -> Self {
        Self { format, confidence }
    }
}

/// Everything a reader receives when asked to parse.
#[derive(Debug, Clone, Copy)]
pub struct Input<'a> {
    /// The complete file contents.
    pub data: &'a [u8],
    /// File name, when known (for diagnostics and titles only).
    pub name: Option<&'a str>,
    /// The format that detection settled on.
    pub format: &'static FormatInfo,
    /// Caller preferences and safety limits.
    pub options: &'a ReadOptions,
}

/// A parser for one family of file formats.
///
/// Implementations must be pure functions of their input: no global state,
/// no I/O besides the provided bytes. They should degrade gracefully, returning
/// whatever content they could recover and describing the rest in
/// [`Document::warnings`].
pub trait Reader: Send + Sync + fmt::Debug {
    /// The formats this reader can parse.
    fn formats(&self) -> &[&'static FormatInfo];

    /// Inspect the input cheaply and report whether it is one of
    /// [`formats`](Self::formats). Must not panic on arbitrary input.
    fn detect(&self, probe: &Probe<'_>) -> Option<Detection>;

    /// Parse the input into a [`Document`].
    fn read(&self, input: &Input<'_>) -> Result<Document>;
}
