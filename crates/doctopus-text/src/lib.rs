//! Plain-text reader for [doctopus](https://crates.io/crates/doctopus).
//!
//! Text files (`.txt`, `.log`, `.csv`, `.md`, `.json`...) become one
//! [`Block::Code`] holding the text exactly as written, so a text file
//! attached to a document is read along with it. Files are recognised by
//! their extension, and only when they hold no NUL bytes, which binary
//! files almost always do.
//!
//! UTF-8 and UTF-16 with a byte order mark are decoded as such; anything
//! else that is not valid UTF-8 is read as Windows-1252, with a warning.
//!
//! ```
//! let mut registry = doctopus_core::Registry::new();
//! doctopus_text::register(&mut registry);
//! assert!(registry.formats().iter().any(|f| f.id == "text"));
//! ```

use doctopus_core::{
    Block, Code, Confidence, Detection, Document, DocumentKind, FormatInfo, Input, Probe, Reader,
    Registry, Result, Section, SectionKind,
};
use encoding_rs::{Encoding, WINDOWS_1252};

/// Plain-text files.
pub static TEXT: FormatInfo = FormatInfo {
    id: "text",
    name: "Plain text",
    kind: DocumentKind::Text,
    extensions: &[
        "txt", "text", "log", "csv", "tsv", "md", "markdown", "json", "yaml", "yml", "ini",
    ],
    mime: "text/plain",
};

/// How much of a file is searched for NUL bytes.
const BINARY_CHECK: usize = 8 << 10;

/// Register the readers of this crate.
pub fn register(registry: &mut Registry) {
    registry.register(TextReader);
}

/// Reads plain-text files.
#[derive(Debug, Default, Clone, Copy)]
pub struct TextReader;

impl Reader for TextReader {
    fn formats(&self) -> &[&'static FormatInfo] {
        static FORMATS: [&FormatInfo; 1] = [&TEXT];
        &FORMATS
    }

    fn detect(&self, probe: &Probe<'_>) -> Option<Detection> {
        let extension = probe.extension()?;
        if !TEXT.extensions.contains(&extension.as_str()) {
            return None;
        }
        let data = probe.data();
        let text =
            Encoding::for_bom(data).is_some() || !data[..data.len().min(BINARY_CHECK)].contains(&0);
        // Only the name says it is text, so any reader that recognises the
        // content wins.
        text.then(|| Detection::new(&TEXT, Confidence::Low))
    }

    fn read(&self, input: &Input<'_>) -> Result<Document> {
        let mut doc = Document::new(&TEXT);
        let (text, warning) = decode(input.data);
        if let Some(warning) = warning {
            doc.warn(warning);
        }
        let mut section = Section::new(SectionKind::Body);
        section.blocks.push(Block::Code(Code {
            language: language(input.name),
            text: text.replace("\r\n", "\n").replace('\r', "\n"),
        }));
        doc.sections.push(section);
        Ok(doc)
    }
}

/// The text of `data`, and a warning when it had to be guessed.
fn decode(data: &[u8]) -> (String, Option<String>) {
    if let Some((encoding, bom)) = Encoding::for_bom(data) {
        let (text, errors) = encoding.decode_without_bom_handling(&data[bom..]);
        let warning = errors.then(|| {
            format!(
                "some bytes are not valid {}; they read as \u{FFFD}",
                encoding.name()
            )
        });
        return (text.into_owned(), warning);
    }
    match std::str::from_utf8(data) {
        Ok(text) => (text.to_string(), None),
        Err(_) => {
            let (text, _) = WINDOWS_1252.decode_without_bom_handling(data);
            let warning = "the text is not UTF-8; it was read as Windows-1252".to_string();
            (text.into_owned(), Some(warning))
        }
    }
}

/// The language of a code block for the file `name`, for syntax
/// highlighting.
fn language(name: Option<&str>) -> Option<String> {
    let (_, extension) = name?.rsplit_once('.')?;
    let language = match extension.to_ascii_lowercase().as_str() {
        "csv" => "csv",
        "tsv" => "tsv",
        "md" | "markdown" => "markdown",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "ini" => "ini",
        _ => return None,
    };
    Some(language.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctopus_core::ReadOptions;

    fn read(data: &[u8], name: &str) -> Option<Document> {
        let probe = Probe::new(data, Some(name));
        let detection = TextReader.detect(&probe)?;
        let input = Input {
            data,
            name: Some(name),
            format: detection.format,
            options: &ReadOptions::default(),
        };
        Some(TextReader.read(&input).unwrap())
    }

    fn code(doc: &Document) -> &Code {
        match &doc.sections[0].blocks[0] {
            Block::Code(code) => code,
            other => panic!("not code: {other:?}"),
        }
    }

    #[test]
    fn text_files_are_kept_as_written() {
        let doc = read(b"line 1\r\nline 2\n", "notes.TXT").unwrap();
        assert_eq!(code(&doc).text, "line 1\nline 2\n");
        assert_eq!(code(&doc).language, None);
        assert!(doc.warnings.is_empty());

        let doc = read(b"a,b\n1,2\n", "data.csv").unwrap();
        assert_eq!(code(&doc).language.as_deref(), Some("csv"));
    }

    #[test]
    fn encodings_are_recognised() {
        let doc = read(b"\xEF\xBB\xBFcaf\xC3\xA9", "a.txt").unwrap();
        assert_eq!(code(&doc).text, "café");

        let doc = read(b"\xFF\xFEc\0a\0f\0\xE9\0", "a.txt").unwrap();
        assert_eq!(code(&doc).text, "café");
        assert!(doc.warnings.is_empty());

        let doc = read(b"caf\xE9", "a.txt").unwrap();
        assert_eq!(code(&doc).text, "café");
        assert_eq!(
            doc.warnings,
            ["the text is not UTF-8; it was read as Windows-1252"]
        );
    }

    #[test]
    fn binary_files_and_other_names_are_not_text() {
        assert!(read(b"MZ\x90\0\x03\0", "setup.txt").is_none());
        assert!(read(b"plain words", "notes.bin").is_none());
        assert!(TextReader.detect(&Probe::new(b"plain", None)).is_none());
    }
}
