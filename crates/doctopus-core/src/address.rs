//! Addresses of documents and of the parts nested inside them.

use std::fmt;
use std::path::{Path, PathBuf};

/// A file plus an optional path of part ids: `report.docx#emb1/img2`.
///
/// The `#` syntax lets a caller reach anything a document contains with a
/// single argument, however deep it is nested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// The file on disk.
    pub file: PathBuf,
    /// Part ids from the outermost document inwards (empty for the file
    /// itself).
    pub parts: Vec<String>,
}

impl Address {
    /// Parse an address, checking the file system so that file names that
    /// themselves contain `#` keep working.
    pub fn parse(input: &str) -> Self {
        Self::parse_with(input, |p| Path::new(p).is_file())
    }

    /// Parse an address using `exists` to decide which prefix names a file.
    ///
    /// The longest prefix that exists wins; if none does, the input is split
    /// at its first `#`.
    pub fn parse_with(input: &str, exists: impl Fn(&str) -> bool) -> Self {
        if !input.contains('#') || exists(input) {
            return Self::file(input);
        }
        let split = input
            .match_indices('#')
            .map(|(i, _)| i)
            .rev()
            .find(|&i| exists(&input[..i]))
            .or_else(|| input.find('#'));
        match split {
            Some(i) => Self {
                file: PathBuf::from(&input[..i]),
                parts: input[i + 1..]
                    .split('/')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect(),
            },
            None => Self::file(input),
        }
    }

    fn file(input: &str) -> Self {
        Self {
            file: PathBuf::from(input),
            parts: Vec::new(),
        }
    }

    /// The same file with one more part id appended.
    pub fn child(&self, id: &str) -> Self {
        let mut parts = self.parts.clone();
        parts.push(id.to_string());
        Self {
            file: self.file.clone(),
            parts,
        }
    }

    /// The part ids as string slices, for [`Document::resolve`](crate::Document::resolve).
    pub fn part_path(&self) -> Vec<&str> {
        self.parts.iter().map(String::as_str).collect()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file.display())?;
        if !self.parts.is_empty() {
            write!(f, "#{}", self.parts.join("/"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_part_paths() {
        let a = Address::parse_with("deck.pptx#emb2/img1", |p| p == "deck.pptx");
        assert_eq!(a.file, PathBuf::from("deck.pptx"));
        assert_eq!(a.parts, ["emb2", "img1"]);
        assert_eq!(a.to_string(), "deck.pptx#emb2/img1");
    }

    #[test]
    fn keeps_hashes_that_belong_to_the_file_name() {
        let a = Address::parse_with("C# notes.docx", |p| p == "C# notes.docx");
        assert!(a.parts.is_empty());
        let b = Address::parse_with("C# notes.docx#img3", |p| p == "C# notes.docx");
        assert_eq!(b.file, PathBuf::from("C# notes.docx"));
        assert_eq!(b.parts, ["img3"]);
    }

    #[test]
    fn falls_back_to_first_hash_for_missing_files() {
        let a = Address::parse_with("missing.docx#emb1", |_| false);
        assert_eq!(a.file, PathBuf::from("missing.docx"));
        assert_eq!(a.parts, ["emb1"]);
    }
}
