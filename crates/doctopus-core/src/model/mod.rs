//! The format-independent document model every reader produces.
//!
//! A [`Document`] is a list of [`Section`]s (the body of a Word file, the
//! sheets of a workbook, the slides of a deck, the pages of a drawing, plus
//! auxiliary sections such as comments or footnotes). Sections hold
//! [`Block`]s; blocks hold [`Inline`]s. Binary payloads — images, embedded
//! files, OLE objects — live in [`Document::parts`] and are referenced from the
//! content by [`PartId`].

mod block;
mod cell;
mod part;

use std::collections::BTreeMap;

use serde::Serialize;

pub use self::block::*;
pub use self::cell::{CellRange, CellRef};
pub use self::part::{Part, PartId, PartKind};
use crate::{Error, FormatInfo, Result};

/// A parsed document.
#[derive(Debug, Clone, Serialize)]
pub struct Document {
    /// The detected format.
    pub format: &'static FormatInfo,
    /// Title, author, dates and other properties.
    #[serde(skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Content, in reading order.
    pub sections: Vec<Section>,
    /// Images, embedded objects and attached files referenced by the content.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<Part>,
    /// Problems that did not prevent reading but may have lost content.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl Document {
    /// An empty document of the given format.
    pub fn new(format: &'static FormatInfo) -> Self {
        Self {
            format,
            metadata: Metadata::default(),
            sections: Vec::new(),
            parts: Vec::new(),
            warnings: Vec::new(),
        }
    }

    /// Record a non-fatal problem. Identical messages are kept once.
    pub fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    /// Attach a part and return its id.
    ///
    /// Ids are assigned per kind in insertion order (`img1`, `img2`, `emb1`,
    /// ...). If a part with the same [`Part::origin`] was already added, that
    /// part's id is returned instead, so content reused in many places (a logo
    /// on every slide) is stored and listed once.
    pub fn add_part(&mut self, mut part: Part) -> PartId {
        if let Some(origin) = part.origin.as_deref()
            && let Some(existing) = self.part_by_origin(origin)
        {
            return existing.id.clone();
        }
        let prefix = part.kind.id_prefix();
        let n = self
            .parts
            .iter()
            .filter(|p| p.kind.id_prefix() == prefix)
            .count()
            + 1;
        part.id = PartId::new(format!("{prefix}{n}"));
        let id = part.id.clone();
        self.parts.push(part);
        id
    }

    /// The part with the given id, at this level only.
    pub fn part(&self, id: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.id.as_str() == id)
    }

    /// Mutable access to the part with the given id, at this level only.
    pub fn part_mut(&mut self, id: &str) -> Option<&mut Part> {
        self.parts.iter_mut().find(|p| p.id.as_str() == id)
    }

    /// The part that was created from the given container location, if any.
    pub fn part_by_origin(&self, origin: &str) -> Option<&Part> {
        self.parts
            .iter()
            .find(|p| p.origin.as_deref() == Some(origin))
    }

    /// Follow a path of part ids through nested documents
    /// (`["emb1", "img2"]` is image 2 inside embedded object 1).
    pub fn resolve(&self, path: &[&str]) -> Result<&Part> {
        let Some((first, rest)) = path.split_first() else {
            return Err(Error::InvalidArgument("empty part path".into()));
        };
        let part = self.part(first).ok_or_else(|| Error::PartNotFound {
            id: (*first).to_string(),
            within: "this document".into(),
            available: self.parts.iter().map(|p| p.id.to_string()).collect(),
        })?;
        if rest.is_empty() {
            return Ok(part);
        }
        match &part.document {
            Some(doc) => doc.resolve(rest).map_err(|err| match err {
                Error::PartNotFound { id, available, .. } => Error::PartNotFound {
                    id,
                    within: format!("part `{first}`"),
                    available,
                },
                other => other,
            }),
            None => Err(Error::PartNotFound {
                id: rest[0].to_string(),
                within: format!("part `{first}` (it has no readable content of its own)"),
                available: Vec::new(),
            }),
        }
    }

    /// Like [`resolve`](Self::resolve), but takes the part out of the
    /// document instead of borrowing it.
    pub fn into_part(mut self, path: &[&str]) -> Result<Part> {
        self.resolve(path)?;
        let (first, rest) = path.split_first().expect("resolve checked the path");
        let index = self
            .parts
            .iter()
            .position(|p| p.id.as_str() == *first)
            .expect("resolve checked the part");
        let part = self.parts.swap_remove(index);
        if rest.is_empty() {
            return Ok(part);
        }
        match part.document {
            Some(child) => child.into_part(rest),
            None => unreachable!("resolve checked the nesting"),
        }
    }

    /// Sections such as sheets, slides and pages that can be selected by
    /// number or name.
    pub fn primary_sections(&self) -> impl Iterator<Item = &Section> {
        self.sections.iter().filter(|s| s.kind.is_primary())
    }
}

/// Descriptive properties of a document. Every field is optional because
/// formats store wildly different subsets.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Metadata {
    /// Document title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Original author.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Person who saved the document last.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified_by: Option<String>,
    /// Creation time, ISO 8601.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// Last modification time, ISO 8601.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    /// Keywords or tags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<String>,
    /// Free-form description or comments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Application that produced the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    /// Company or organisation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    /// Everything else: statistics (`pages`, `words`), custom properties, ...
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

impl Metadata {
    /// True when no property is set.
    pub fn is_empty(&self) -> bool {
        self.fields().next().is_none()
    }

    /// All set properties as `(name, value)` pairs, well-known ones first.
    pub fn fields(&self) -> impl Iterator<Item = (&str, &str)> {
        let known = [
            ("title", &self.title),
            ("subject", &self.subject),
            ("author", &self.author),
            ("last_modified_by", &self.last_modified_by),
            ("created", &self.created),
            ("modified", &self.modified),
            ("keywords", &self.keywords),
            ("description", &self.description),
            ("application", &self.application),
            ("company", &self.company),
        ];
        known
            .into_iter()
            .filter_map(|(k, v)| v.as_deref().map(|v| (k, v)))
            .chain(self.extra.iter().map(|(k, v)| (k.as_str(), v.as_str())))
    }

    /// Set a property from untrusted input, ignoring blank values.
    pub fn set(&mut self, name: &str, value: impl AsRef<str>) {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return;
        }
        let value = value.to_string();
        let slot = match name {
            "title" => &mut self.title,
            "subject" => &mut self.subject,
            "author" => &mut self.author,
            "last_modified_by" => &mut self.last_modified_by,
            "created" => &mut self.created,
            "modified" => &mut self.modified,
            "keywords" => &mut self.keywords,
            "description" => &mut self.description,
            "application" => &mut self.application,
            "company" => &mut self.company,
            _ => {
                self.extra.insert(name.to_string(), value);
                return;
            }
        };
        *slot = Some(value);
    }
}

/// A top-level division of a document.
#[derive(Debug, Clone, Serialize)]
pub struct Section {
    /// What the section represents.
    pub kind: SectionKind,
    /// 1-based position among sections of the same kind (slide 3, sheet 2).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// Sheet name, slide title or page name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Hidden sheets and slides are still read but marked.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// Extra facts worth showing next to the heading (`"range" = "A1:F20"`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, String>,
    /// The content.
    pub blocks: Vec<Block>,
}

impl Section {
    /// An empty section.
    pub fn new(kind: SectionKind) -> Self {
        Self {
            kind,
            number: None,
            name: None,
            hidden: false,
            properties: BTreeMap::new(),
            blocks: Vec::new(),
        }
    }

    /// Builder-style setter for [`Section::number`].
    pub fn with_number(mut self, number: u32) -> Self {
        self.number = Some(number);
        self
    }

    /// Builder-style setter for [`Section::name`]; blank names are ignored.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        let name = name.trim();
        if !name.is_empty() {
            self.name = Some(name.to_string());
        }
        self
    }
}

/// What a [`Section`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    /// Main text of a word-processing document.
    Body,
    /// One worksheet.
    Sheet,
    /// One slide, including its speaker notes.
    Slide,
    /// One page of a drawing.
    Page,
    /// Page headers and footers.
    HeadersFooters,
    /// Footnote texts.
    Footnotes,
    /// Endnote texts.
    Endnotes,
    /// Review comments.
    Comments,
    /// Anything else; [`Section::name`] says what.
    Other,
}

impl SectionKind {
    /// Sheets, slides and pages: the sections a user can select by number.
    pub fn is_primary(self) -> bool {
        matches!(self, Self::Sheet | Self::Slide | Self::Page)
    }

    /// Singular noun used in headings and messages.
    pub fn noun(self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::Sheet => "sheet",
            Self::Slide => "slide",
            Self::Page => "page",
            Self::HeadersFooters => "headers and footers",
            Self::Footnotes => "footnotes",
            Self::Endnotes => "endnotes",
            Self::Comments => "comments",
            Self::Other => "section",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocumentKind;

    static FORMAT: FormatInfo = FormatInfo {
        id: "test",
        name: "Test document",
        kind: DocumentKind::Other,
        extensions: &[],
        mime: "application/x-test",
    };

    /// A document with an image and an object that holds another image.
    fn nested() -> Document {
        let mut inner = Document::new(&FORMAT);
        inner.add_part(Part::new(PartKind::Image, b"inner".to_vec()));
        let mut object = Part::new(PartKind::Object, b"object".to_vec());
        object.document = Some(Box::new(inner));
        let mut outer = Document::new(&FORMAT);
        outer.add_part(Part::new(PartKind::Image, b"outer".to_vec()));
        outer.add_part(object);
        outer
    }

    #[test]
    fn parts_are_resolved_and_taken_through_nested_documents() {
        let doc = nested();
        assert_eq!(doc.resolve(&["emb1", "img1"]).unwrap().data, b"inner");
        let object = nested().into_part(&["emb1"]).unwrap();
        assert!(object.document.is_some());
        assert_eq!(
            nested().into_part(&["emb1", "img1"]).unwrap().data,
            b"inner"
        );
        match nested().into_part(&["emb1", "img9"]) {
            Err(Error::PartNotFound { id, available, .. }) => {
                assert_eq!(id, "img9");
                assert_eq!(available, ["img1"]);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(nested().into_part(&["img1", "img1"]).is_err());
    }
}
