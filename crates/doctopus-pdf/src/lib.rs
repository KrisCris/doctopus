//! PDF reader for [doctopus](https://crates.io/crates/doctopus).
//!
//! Every page becomes a [`SectionKind::Page`] section with its text in
//! reading order, as paragraphs, list items and headings, and its
//! pictures. Links, bookmarks, page labels, form field values, metadata and
//! attached files are read too. Attached files become parts, so an
//! attached Word document is read by the Word reader.
//!
//! ```
//! let mut registry = doctopus_core::Registry::new();
//! doctopus_pdf::register(&mut registry);
//! assert!(registry.formats().iter().any(|f| f.id == "pdf"));
//! ```

mod catalog;
mod decode;
mod image;
mod layout;
mod page;
mod strings;
mod xmp;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use doctopus_core::{
    Block, Detection, Document, DocumentKind, EmbedRef, Error, FormatInfo, Input, PartId, Probe,
    Reader, Registry, Result, Section, SectionKind,
};
use hayro_interpret::{InterpreterCache, InterpreterSettings, InterpreterWarning};
use hayro_syntax::{LoadPdfError, Pdf};

use crate::catalog::Catalog;

/// PDF documents.
pub static PDF: FormatInfo = FormatInfo {
    id: "pdf",
    name: "PDF document",
    kind: DocumentKind::FixedLayout,
    extensions: &["pdf"],
    mime: "application/pdf",
};

/// How far into a file the `%PDF-` signature may start. Readers accept
/// some bytes in front of it, and so does this one.
const SIGNATURE_WINDOW: usize = 1024;
/// Pages listed by name in a warning at most.
const LISTED_PAGES: usize = 10;

/// Register the readers of this crate.
pub fn register(registry: &mut Registry) {
    registry.register(PdfReader);
}

/// Reads PDF documents.
#[derive(Debug, Default, Clone, Copy)]
pub struct PdfReader;

impl Reader for PdfReader {
    fn formats(&self) -> &[&'static FormatInfo] {
        static FORMATS: [&FormatInfo; 1] = [&PDF];
        &FORMATS
    }

    fn detect(&self, probe: &Probe<'_>) -> Option<Detection> {
        let data = probe.data();
        let head = &data[..data.len().min(SIGNATURE_WINDOW)];
        head.windows(5)
            .any(|w| w == b"%PDF-")
            .then(|| Detection::certain(&PDF))
    }

    fn read(&self, input: &Input<'_>) -> Result<Document> {
        read(input)
    }
}

fn read(input: &Input<'_>) -> Result<Document> {
    let format = input.format.name;
    let pdf = Pdf::new(input.data.to_vec()).map_err(|err| match err {
        LoadPdfError::Decryption(_) => Error::encrypted(format),
        // hayro reports some encrypted files it cannot open (a damaged
        // cross-reference table, some AES-256 files) as invalid.
        _ if names_encryption(input.data) => Error::encrypted(format),
        _ => Error::malformed(format, "no pages could be found in it"),
    })?;
    let limit = input.options.max_part_size;
    decode::check(&pdf, limit).map_err(|message| Error::malformed(format, message))?;

    let mut doc = Document::new(input.format);
    let catalog = Catalog::new(&pdf);
    metadata(&pdf, &catalog, &mut doc, limit);

    let unread_fonts = Arc::new(AtomicUsize::new(0));
    let sink = Arc::clone(&unread_fonts);
    let settings = InterpreterSettings {
        // Form fields are listed with their values; annotation appearances
        // would repeat them.
        render_annotations: false,
        warning_sink: Arc::new(move |warning| {
            if matches!(warning, InterpreterWarning::UnsupportedFont) {
                sink.fetch_add(1, Ordering::Relaxed);
            }
        }),
        ..InterpreterSettings::default()
    };
    let cache = InterpreterCache::new();
    let mut budget = page::Budget {
        image_bytes: limit.saturating_mul(2),
        limit,
    };
    let mut pages = Vec::new();
    let mut attached: Vec<Vec<PartId>> = Vec::new();
    let mut notes = Notes::default();
    for (i, page) in pdf.pages().iter().enumerate() {
        let number = i as u32 + 1;
        let annotations = catalog.annotations(page.raw());
        let (marks, report) = page::read(
            page,
            number,
            &annotations.links,
            &cache,
            &settings,
            &mut doc,
            &mut budget,
        );
        let crop = page.intersected_crop_box();
        let links = annotations.links.into_iter().map(|l| l.url).collect();
        pages.push(layout::page(
            marks,
            links,
            crop.y1.max(crop.y0),
            crop.y0.min(crop.y1),
        ));
        let mut files = Vec::new();
        for spec in &annotations.files {
            match catalog::attachment(spec, None, limit) {
                Ok(part) => files.push(doc.add_part(part.with_location(format!("page {number}")))),
                Err(message) => doc.warn(format!("page {number}: {message}")),
            }
        }
        attached.push(files);
        notes.page(number, report, &mut doc);
    }
    notes.finish(&mut doc, unread_fonts.load(Ordering::Relaxed));

    let labels = catalog.page_labels(pages.len());
    let (blocks, running) = layout::blocks(pages);
    for (i, (mut blocks, files)) in blocks.into_iter().zip(attached).enumerate() {
        blocks.extend(
            files
                .into_iter()
                .map(|part| Block::Embed(EmbedRef { part })),
        );
        let mut section = Section::new(SectionKind::Page).with_number(i as u32 + 1);
        if let Some(Some(label)) = labels.get(i) {
            section.properties.insert("label".into(), label.clone());
        }
        section.blocks = blocks;
        doc.sections.push(section);
    }
    if !running.is_empty() {
        let mut section = Section::new(SectionKind::HeadersFooters);
        section.blocks = running.into_iter().map(Block::text).collect();
        doc.sections.push(section);
    }
    let outline = catalog.outline();
    if !outline.is_empty() {
        let mut section = Section::new(SectionKind::Other).with_name("Bookmarks");
        section.blocks = outline;
        doc.sections.push(section);
    }
    if let Some(table) = catalog.form_fields() {
        let mut section = Section::new(SectionKind::Other).with_name("Form fields");
        section.blocks.push(Block::Table(table));
        doc.sections.push(section);
    }
    for (key, spec) in catalog.embedded_files() {
        match catalog::attachment(&spec, Some(&key), limit) {
            Ok(part) => {
                doc.add_part(part);
            }
            Err(message) => doc.warn(message),
        }
    }
    Ok(doc)
}

/// Document properties: the information dictionary first, XMP metadata
/// for what it leaves out.
fn metadata(pdf: &Pdf, catalog: &Catalog<'_>, doc: &mut Document, limit: u64) {
    let info = pdf.metadata();
    let text = |value: &Option<Vec<u8>>| value.as_deref().map(strings::text);
    let fields = [
        ("title", text(&info.title)),
        ("author", text(&info.author)),
        ("subject", text(&info.subject)),
        ("keywords", text(&info.keywords)),
        ("application", text(&info.creator)),
        ("producer", text(&info.producer)),
        ("created", info.creation_date.as_ref().map(strings::date)),
        (
            "modified",
            info.modification_date.as_ref().map(strings::date),
        ),
    ];
    for (name, value) in fields {
        if let Some(value) = value {
            doc.metadata.set(name, value);
        }
    }
    if let Some(stream) = catalog.metadata()
        && let Ok(packet) = decode::decoded(&stream, limit.min(16 << 20))
    {
        for (name, value) in xmp::properties(&packet) {
            if !doc.metadata.fields().any(|(n, _)| n == name) {
                doc.metadata.set(name, value);
            }
        }
    }
    if let Some(language) = catalog.language() {
        doc.metadata.set("language", language);
    }
    let version = format!("{:?}", pdf.version());
    if let Some(digits) = version.strip_prefix("Pdf")
        && let [major, minor] = digits.as_bytes()
    {
        doc.metadata.set(
            "pdf_version",
            format!("{}.{}", char::from(*major), char::from(*minor)),
        );
    }
}

/// Whether a trailer of the file has an `/Encrypt` entry.
fn names_encryption(data: &[u8]) -> bool {
    data.windows(8).enumerate().any(|(at, w)| {
        w == b"/Encrypt"
            && data[at + 8..]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| b.is_ascii_digit() || *b == b'<')
    })
}

/// Problems found on pages, gathered into a few warnings.
#[derive(Debug, Default)]
struct Notes {
    /// Pages with no text but pictures: scans, or text set as a picture.
    pictured: Vec<(u32, Vec<PartId>)>,
    /// Pages with no text but many vector paths: text drawn as outlines.
    outlined: Vec<u32>,
    /// Pages and how many of their glyphs map to no character.
    unmapped: Vec<(u32, usize)>,
}

impl Notes {
    fn page(&mut self, number: u32, report: page::Report, doc: &mut Document) {
        if let Some(failure) = report.failure {
            doc.warn(format!(
                "page {number} could not be read completely: {failure}"
            ));
        }
        if report.cut {
            doc.warn(format!("page {number} has too much text; it was cut off"));
        }
        for reason in dedup(report.skipped) {
            doc.warn(format!(
                "page {number}: a picture was left out because {reason}"
            ));
        }
        if report.unmapped > 0 {
            self.unmapped.push((number, report.unmapped));
        }
        if report.glyphs == 0 {
            if !report.images.is_empty() {
                self.pictured.push((number, dedup(report.images)));
            } else if report.paths >= 50 && report.unmapped == 0 {
                self.outlined.push(number);
            }
        }
    }

    fn finish(self, doc: &mut Document, unread_fonts: usize) {
        if !self.pictured.is_empty() {
            let listed: Vec<String> = self
                .pictured
                .iter()
                .take(LISTED_PAGES)
                .map(|(page, images)| {
                    let ids: Vec<&str> = images.iter().map(PartId::as_str).collect();
                    format!("page {page} ({})", ids.join(", "))
                })
                .collect();
            doc.warn(format!(
                "{} {} no text layer, only pictures (scans, or text set as pictures); extract the pictures to see them: {}{}",
                pages(self.pictured.len()),
                if self.pictured.len() == 1 {
                    "has"
                } else {
                    "have"
                },
                listed.join(", "),
                more(self.pictured.len()),
            ));
        }
        if !self.outlined.is_empty() {
            doc.warn(format!(
                "{} {} no text layer: {} text, if any, is drawn as shapes: {}{}",
                pages(self.outlined.len()),
                if self.outlined.len() == 1 {
                    "has"
                } else {
                    "have"
                },
                if self.outlined.len() == 1 {
                    "its"
                } else {
                    "their"
                },
                list(self.outlined.iter().map(u32::to_string)),
                more(self.outlined.len()),
            ));
        }
        if !self.unmapped.is_empty() {
            let total: usize = self.unmapped.iter().map(|(_, n)| n).sum();
            doc.warn(format!(
                "{total} characters could not be read because their fonts do not say which characters they are; on {}: {}{}",
                pages(self.unmapped.len()),
                list(self.unmapped.iter().map(|(page, _)| page.to_string())),
                more(self.unmapped.len()),
            ));
        }
        if unread_fonts > 0 {
            doc.warn(format!(
                "{unread_fonts} {} could not be read; some text may be missing",
                if unread_fonts == 1 { "font" } else { "fonts" },
            ));
        }
    }
}

fn pages(n: usize) -> String {
    if n == 1 {
        "1 page".into()
    } else {
        format!("{n} pages")
    }
}

fn list(items: impl Iterator<Item = String>) -> String {
    items.take(LISTED_PAGES).collect::<Vec<_>>().join(", ")
}

fn more(n: usize) -> String {
    match n.saturating_sub(LISTED_PAGES) {
        0 => String::new(),
        rest => format!(" and {rest} more"),
    }
}

/// Items in order of first appearance, each once.
fn dedup<T: PartialEq>(items: Vec<T>) -> Vec<T> {
    let mut out: Vec<T> = Vec::with_capacity(items.len());
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signature_is_found_near_the_start() {
        let detect = |data: &[u8]| PdfReader.detect(&Probe::new(data, None)).is_some();
        assert!(detect(b"%PDF-1.7\n"));
        assert!(detect(b"\xef\xbb\xbfjunk\r\n%PDF-1.4"));
        let mut late = vec![b' '; SIGNATURE_WINDOW];
        late.extend_from_slice(b"%PDF-1.4");
        assert!(!detect(&late));
        assert!(!detect(b"%PS-Adobe-3.0"));
        assert!(!detect(b""));
    }

    #[test]
    fn encryption_entries_are_found() {
        assert!(names_encryption(
            b"trailer << /Root 1 0 R /Encrypt 9 0 R >>"
        ));
        assert!(names_encryption(b"/Encrypt\n<< /Filter /Standard >>"));
        assert!(!names_encryption(b"<< /EncryptMetadata false >>"));
        assert!(!names_encryption(b"/Encrypt"));
    }
}
