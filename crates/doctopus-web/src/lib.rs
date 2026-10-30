//! doctopus in the browser: the WebAssembly half of the web app in `www/`.
//!
//! The page hands a document's bytes to [`Conversion::new`], which converts
//! it as `doctopus convert` does, but in memory: `NAME.md`, `NAME.parts/`
//! and the Markdown of every embedded document. The page then asks for the
//! list of files, their bytes, a Markdown file as HTML for its viewer, and
//! the whole tree as a zip file. Nothing leaves the browser.
//!
//! Build the page with `scripts/build-web.sh`.

mod html;

use std::collections::BTreeMap;
use std::io::{Cursor, Write as _};
use std::path::Path;
use std::sync::Mutex;

use doctopus_core::render::{self, RenderOptions, describe_part};
use doctopus_core::{Address, Engine, ReadOptions, Registry};
use serde::Serialize;
use wasm_bindgen::prelude::*;
use zip::write::SimpleFileOptions;

/// Levels of embedded documents to parse, as in the command line.
const MAX_DEPTH: u32 = 32;

/// A document converted to linked files, kept for the page to read.
#[wasm_bindgen]
#[derive(Debug)]
pub struct Conversion {
    manifest: Manifest,
    /// File contents by path.
    contents: BTreeMap<String, Vec<u8>>,
}

/// What the conversion holds, for the page.
#[derive(Debug, Serialize)]
struct Manifest {
    /// The converted file's name.
    source: String,
    /// `NAME` of `NAME.md` and `NAME.parts`.
    name: String,
    /// Markdown files (the document's own first, then those of embedded
    /// documents, depth first), then the files they link to.
    files: Vec<Entry>,
    /// Parts with nothing to write.
    skipped: Vec<Skipped>,
    /// Warnings in all converted documents.
    warnings: usize,
}

/// One file of the conversion.
#[derive(Debug, Serialize)]
struct Entry {
    /// Relative to the output directory, with `/` between components.
    path: String,
    /// The address of the document or part it holds.
    address: String,
    /// Whether it is the Markdown of a document.
    markdown: bool,
    /// Format id of the converted document, for Markdown files.
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'static str>,
    /// Media type, for other files.
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    /// Size in bytes.
    size: usize,
    /// Estimated tokens, for Markdown files.
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<usize>,
    /// One-line description.
    summary: String,
}

/// A part with nothing to write.
#[derive(Debug, Serialize)]
struct Skipped {
    address: String,
    reason: String,
}

#[wasm_bindgen]
impl Conversion {
    /// Convert the file `file_name` holding `data`. Fails with the message
    /// the command line would print when the file cannot be read.
    #[wasm_bindgen(constructor)]
    pub fn new(data: &[u8], file_name: &str) -> Result<Conversion, String> {
        let document = engine()
            .parse(data, Some(file_name))
            .map_err(|err| err.to_string())?;
        let name = stem(file_name);
        let options = RenderOptions::new(Address::parse_with(file_name, |_| true));
        let converted = render::convert(&document, &name, &options);
        let mut files = Vec::new();
        let mut contents = BTreeMap::new();
        for markdown in converted.markdown {
            files.push(Entry {
                path: markdown.path.clone(),
                address: markdown.address.to_string(),
                markdown: true,
                format: Some(markdown.document.format.id),
                content_type: None,
                size: markdown.text.len(),
                tokens: Some(markdown.tokens()),
                summary: markdown.document.format.name.to_string(),
            });
            contents.insert(markdown.path, markdown.text.into_bytes());
        }
        for file in converted.files {
            files.push(Entry {
                path: file.path.clone(),
                address: file.address.to_string(),
                markdown: false,
                format: None,
                content_type: file.part.content_type.clone(),
                size: file.part.data.len(),
                tokens: None,
                summary: describe_part(file.part),
            });
            contents.insert(file.path, file.part.data.clone());
        }
        let skipped = converted
            .skipped
            .into_iter()
            .map(|skipped| Skipped {
                address: skipped.address.to_string(),
                reason: skipped.reason,
            })
            .collect();
        Ok(Self {
            manifest: Manifest {
                source: file_name.to_string(),
                name,
                files,
                skipped,
                warnings: converted.warnings,
            },
            contents,
        })
    }

    /// The files and what they hold, as JSON.
    pub fn manifest(&self) -> String {
        serde_json::to_string(&self.manifest).unwrap_or_default()
    }

    /// The content of the file at `path`.
    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.contents.get(path).cloned()
    }

    /// The Markdown file at `path` as HTML for the viewer; see the `html`
    /// module for what it holds.
    pub fn html(&self, path: &str) -> Option<String> {
        let markdown = String::from_utf8_lossy(self.contents.get(path)?);
        Some(html::to_html(&markdown, path, |p| {
            self.contents.contains_key(p)
        }))
    }

    /// Every file in one zip file, the Markdown of the document first.
    pub fn zip(&self) -> Result<Vec<u8>, String> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        for entry in &self.manifest.files {
            zip.start_file(entry.path.as_str(), options)
                .map_err(|err| err.to_string())?;
            zip.write_all(&self.contents[&entry.path])
                .map_err(|err| err.to_string())?;
        }
        let zip = zip.finish().map_err(|err| err.to_string())?;
        Ok(zip.into_inner())
    }
}

/// The formats this build reads, as JSON: `[{"name": ..., "extensions": [...]}]`.
#[wasm_bindgen]
pub fn formats() -> String {
    #[derive(Serialize)]
    struct Format {
        name: &'static str,
        extensions: &'static [&'static str],
    }
    let formats: Vec<Format> = registry()
        .formats()
        .into_iter()
        .map(|f| Format {
            name: f.name,
            extensions: f.extensions,
        })
        .collect();
    serde_json::to_string(&formats).unwrap_or_default()
}

/// The doctopus version.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The message of the last panic, for [`last_panic`].
static PANIC: Mutex<Option<String>> = Mutex::new(None);

/// Keep the message of a panic for [`last_panic`]. In WebAssembly a panic
/// aborts instead of unwinding, so the engine cannot turn it into an error
/// for the one part that caused it: the call fails as a whole, and the page
/// asks why.
#[wasm_bindgen(start)]
pub fn start() {
    std::panic::set_hook(Box::new(|info| {
        if let Ok(mut panic) = PANIC.lock() {
            *panic = Some(info.to_string());
        }
    }));
}

/// The message of the panic that stopped the last call, if one did.
#[wasm_bindgen]
pub fn last_panic() -> Option<String> {
    PANIC.lock().ok()?.take()
}

/// Every reader, in the command line's order.
fn registry() -> Registry {
    let mut registry = Registry::new();
    doctopus_office::register(&mut registry);
    doctopus_pdf::register(&mut registry);
    // Last: both only claim files that no other reader recognises.
    doctopus_archive::register(&mut registry);
    doctopus_text::register(&mut registry);
    registry
}

fn engine() -> Engine {
    let mut options = ReadOptions::default();
    options.max_depth = MAX_DEPTH;
    Engine::new(registry()).with_options(options)
}

/// `NAME` for a file: its name without the extension, as the command line
/// names it.
fn stem(file_name: &str) -> String {
    Path::new(file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "document".into())
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::*;

    #[test]
    fn converts_in_memory_and_zips_every_file() {
        let text = "# Notes <b>\n";
        let conversion = Conversion::new(text.as_bytes(), "notes.txt").unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&conversion.manifest()).unwrap();
        assert_eq!(manifest["name"], "notes");
        assert_eq!(manifest["files"][0]["path"], "notes.md");
        assert_eq!(manifest["files"][0]["markdown"], true);
        let markdown = String::from_utf8(conversion.file("notes.md").unwrap()).unwrap();
        assert!(
            markdown.starts_with("---\nsource: notes.txt\n"),
            "{markdown}"
        );
        let html = conversion.html("notes.md").unwrap();
        // Text files are kept exactly as written, in a code block.
        assert!(html.contains("<pre><code># Notes &lt;b&gt;\n"), "{html}");
        assert!(conversion.html("missing.md").is_none());

        let zip = conversion.zip().unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(zip)).unwrap();
        assert_eq!(archive.len(), 1);
        let mut unzipped = String::new();
        archive
            .by_name("notes.md")
            .unwrap()
            .read_to_string(&mut unzipped)
            .unwrap();
        assert_eq!(unzipped, markdown);
    }

    #[test]
    fn unreadable_files_fail_with_the_reason() {
        let err = Conversion::new(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1", "x.doc").unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(stem("report.final.docx"), "report.final");
        assert_eq!(stem(".docx"), ".docx");
        assert!(formats().contains("\"docx\""));
    }
}
