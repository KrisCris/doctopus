//! Zip archive reader for [doctopus](https://crates.io/crates/doctopus).
//!
//! Every file in an archive becomes a [`Part`] named after its path in the
//! archive, and the document lists them in archive order. The engine then
//! reads each part with its own reader, so a Word document in a zip, or a
//! zip in a zip, is read like any other embedded document.
//!
//! Office Open XML files are zip archives too; their readers recognise them
//! with more confidence, so they win. Other zip-based formats (OpenDocument,
//! EPUB, Java archives) are left alone: this reader only takes data named
//! `.zip` or not named at all, so their files are not listed as an archive.
//!
//! ```
//! let mut registry = doctopus_core::Registry::new();
//! doctopus_archive::register(&mut registry);
//! assert!(registry.formats().iter().any(|f| f.id == "zip"));
//! ```

use doctopus_core::container::Zip;
use doctopus_core::{
    Block, Confidence, Detection, Document, DocumentKind, EmbedRef, Error, FormatInfo, Input, Part,
    PartKind, Probe, Reader, Registry, Result, Section, SectionKind,
};

/// Zip archives.
pub static ZIP: FormatInfo = FormatInfo {
    id: "zip",
    name: "Zip archive",
    kind: DocumentKind::Archive,
    extensions: &["zip"],
    mime: "application/zip",
};

/// Files read from one archive at most.
const MAX_FILES: usize = 10_000;

/// Register the readers of this crate.
pub fn register(registry: &mut Registry) {
    registry.register(ZipReader);
}

/// Reads zip archives.
#[derive(Debug, Default, Clone, Copy)]
pub struct ZipReader;

impl Reader for ZipReader {
    fn formats(&self) -> &[&'static FormatInfo] {
        static FORMATS: [&FormatInfo; 1] = [&ZIP];
        &FORMATS
    }

    fn detect(&self, probe: &Probe<'_>) -> Option<Detection> {
        let named = probe.extension().is_none_or(|e| e == "zip");
        (named && probe.is_zip()).then(|| Detection::new(&ZIP, Confidence::Low))
    }

    fn read(&self, input: &Input<'_>) -> Result<Document> {
        read(input)
    }
}

fn read(input: &Input<'_>) -> Result<Document> {
    let max_size = input.options.max_part_size;
    let mut zip = Zip::new(input.data, max_size)
        .map_err(|err| Error::malformed(ZIP.name, err.to_string()))?;
    let mut doc = Document::new(&ZIP);
    if zip.is_recovered() {
        doc.warn("the archive is damaged; files after the damage are missing");
    }
    let files: Vec<String> = zip
        .names()
        .iter()
        .filter(|name| !name.ends_with('/'))
        .cloned()
        .collect();
    if files.len() > MAX_FILES {
        doc.warn(format!(
            "only the first {MAX_FILES} of {} files were read",
            files.len()
        ));
    }
    let mut section = Section::new(SectionKind::Body);
    let mut total = 0u64;
    for name in files.iter().take(MAX_FILES) {
        let data = match zip.read(name) {
            Ok(Some(data)) => data,
            Ok(None) => continue,
            Err(err) => {
                doc.warn(format!("`{name}` could not be read: {err}"));
                continue;
            }
        };
        total += data.len() as u64;
        if total > max_size {
            doc.warn(format!(
                "stopped at `{name}`: the files unpack to more than {max_size} bytes"
            ));
            break;
        }
        let part = Part::new(PartKind::File, data)
            .with_name(name.as_str())
            .with_origin(name.as_str());
        let id = doc.add_part(part);
        section.blocks.push(Block::Embed(EmbedRef { part: id }));
    }
    doc.sections.push(section);
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use doctopus_core::{Engine, ReadOptions};

    use super::*;

    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.add_directory("docs/", stored).unwrap();
        for (name, data) in files {
            writer.start_file(*name, stored).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn engine() -> Engine {
        let mut registry = Registry::new();
        register(&mut registry);
        doctopus_text::register(&mut registry);
        Engine::new(registry)
    }

    #[test]
    fn files_become_parts_read_by_their_own_readers() {
        let inner = zip(&[("deep.txt", b"at the bottom")]);
        let outer = zip(&[("docs/notes.txt", b"hello"), ("inner.zip", &inner)]);
        let doc = engine().parse(&outer, Some("bundle.zip")).unwrap();
        assert_eq!(doc.format.id, "zip");
        let names: Vec<_> = doc.parts.iter().map(|p| p.name.as_deref()).collect();
        assert_eq!(names, [Some("docs/notes.txt"), Some("inner.zip")]);
        assert_eq!(doc.sections[0].blocks.len(), 2);

        let notes = doc.parts[0].document.as_ref().unwrap();
        assert_eq!(notes.format.id, "text");
        let inner = doc.parts[1].document.as_ref().unwrap();
        let deep = inner.parts[0].document.as_ref().unwrap();
        assert_eq!(deep.format.id, "text");
    }

    #[test]
    fn only_archives_named_zip_or_unnamed_are_taken() {
        let data = zip(&[("mimetype", b"application/vnd.oasis.opendocument.text")]);
        let detect = |name| ZipReader.detect(&Probe::new(&data, name));
        assert!(detect(Some("a.ZIP")).is_some());
        assert!(detect(None).is_some());
        assert!(detect(Some("a.odt")).is_none());
        assert!(ZipReader.detect(&Probe::new(b"plain", None)).is_none());
    }

    #[test]
    fn archives_larger_than_the_limit_are_cut_short() {
        let data = zip(&[("a.txt", &[b'a'; 60]), ("b.txt", &[b'b'; 60])]);
        let mut options = ReadOptions::default();
        options.max_part_size = 100;
        let input = Input {
            data: &data,
            name: None,
            format: &ZIP,
            options: &options,
        };
        let doc = ZipReader.read(&input).unwrap();
        assert_eq!(doc.parts.len(), 1);
        assert_eq!(
            doc.warnings,
            ["stopped at `b.txt`: the files unpack to more than 100 bytes"]
        );
    }
}
