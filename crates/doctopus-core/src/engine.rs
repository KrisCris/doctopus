//! Detection, dispatch and recursive parsing of embedded content.

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use crate::sniff::{image_dimensions, sniff};
use crate::{Document, Error, Input, PartKind, Probe, ReadOptions, Reader, Registry, Result};

/// Turns files into [`Document`]s using the readers of a [`Registry`].
///
/// After a reader returns, the engine walks the document's parts: payloads
/// that some reader recognises (a workbook inside a slide, a document inside
/// that workbook...) are parsed too and attached as
/// [`Part::document`](crate::Part::document), up to
/// [`ReadOptions::max_depth`] levels. Parts no reader understands keep their
/// bytes and a sniffed media type, so callers can extract them for other
/// tools.
#[derive(Debug)]
pub struct Engine {
    registry: Registry,
    options: ReadOptions,
}

impl Engine {
    /// An engine with default options.
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            options: ReadOptions::default(),
        }
    }

    /// Replace the options.
    pub fn with_options(mut self, options: ReadOptions) -> Self {
        self.options = options;
        self
    }

    /// The options in effect.
    pub fn options(&self) -> &ReadOptions {
        &self.options
    }

    /// The readers this engine dispatches to.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Read and parse a file.
    pub fn open(&self, path: impl AsRef<Path>) -> Result<Document> {
        let path = path.as_ref();
        let data = std::fs::read(path).map_err(|source| Error::Io {
            path: path.display().to_string(),
            source,
        })?;
        let name = path.file_name().and_then(|n| n.to_str());
        self.parse(&data, name)
    }

    /// Parse in-memory bytes. `name` is an optional file name used as a
    /// detection hint and for messages.
    pub fn parse(&self, data: &[u8], name: Option<&str>) -> Result<Document> {
        let mut doc = self.parse_single(data, name)?;
        let budget = Cell::new(self.options.max_documents);
        self.expand(&mut doc, 1, &budget);
        Ok(doc)
    }

    fn parse_single(&self, data: &[u8], name: Option<&str>) -> Result<Document> {
        let probe = Probe::new(data, name);
        let Some((reader, detection)) = self.registry.detect(&probe) else {
            return Err(Error::Unsupported {
                detected: sniff(data).map(|s| s.description.to_string()),
            });
        };
        let input = Input {
            data,
            name,
            format: detection.format,
            options: &self.options,
        };
        guarded(reader, &input)
    }

    fn expand(&self, doc: &mut Document, depth: u32, budget: &Cell<u32>) {
        for part in &mut doc.parts {
            if part.content_type.is_none() {
                part.content_type = sniff(&part.data).map(|s| s.mime.to_string());
            }
            if part.kind == PartKind::Image && part.dimensions.is_none() {
                part.dimensions = image_dimensions(&part.data);
            }
            if part.data.is_empty() || part.document.is_some() || depth > self.options.max_depth {
                continue;
            }
            let probe = Probe::new(&part.data, part.name.as_deref());
            let Some((reader, detection)) = self.registry.detect(&probe) else {
                continue;
            };
            if budget.get() == 0 {
                part.error = Some("not parsed: too many embedded documents".into());
                continue;
            }
            budget.set(budget.get() - 1);
            let input = Input {
                data: &part.data,
                name: part.name.as_deref(),
                format: detection.format,
                options: &self.options,
            };
            match guarded(reader, &input) {
                Ok(mut child) => {
                    self.expand(&mut child, depth + 1, budget);
                    if part.kind == PartKind::Other {
                        part.kind = PartKind::Object;
                    }
                    part.content_type = Some(detection.format.mime.to_string());
                    part.document = Some(Box::new(child));
                }
                Err(err) => part.error = Some(err.to_string()),
            }
        }
    }
}

/// Run a reader, turning a panic (a bug triggered by hostile or damaged
/// input) into an ordinary error so one bad embedded object cannot take down
/// the whole document.
fn guarded(reader: &dyn Reader, input: &Input<'_>) -> Result<Document> {
    match panic::catch_unwind(AssertUnwindSafe(|| reader.read(input))) {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            Err(Error::malformed(
                input.format.name,
                format!("reader crashed ({message}); please report this file"),
            ))
        }
    }
}
