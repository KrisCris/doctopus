//! Cheap, shared inspection of an input for format detection.

use std::sync::OnceLock;

use crate::container::{Cfb, Zip};

const DETECTION_READ_LIMIT: u64 = 4 << 20;

/// What a [`Reader`](crate::Reader) sees during detection.
///
/// The expensive facts (zip entry names, compound file entries) are computed
/// on first use and shared by every reader that asks, so detection stays
/// cheap however many readers are registered.
#[derive(Debug)]
pub struct Probe<'a> {
    data: &'a [u8],
    name: Option<&'a str>,
    zip_names: OnceLock<Vec<String>>,
    cfb_names: OnceLock<Vec<String>>,
}

impl<'a> Probe<'a> {
    /// Probe `data`; `name` is an optional file name used as a hint.
    pub fn new(data: &'a [u8], name: Option<&'a str>) -> Self {
        Self {
            data,
            name,
            zip_names: OnceLock::new(),
            cfb_names: OnceLock::new(),
        }
    }

    /// The complete input.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// The file name hint, if any.
    pub fn name(&self) -> Option<&'a str> {
        self.name
    }

    /// Lower-case file extension of the name hint.
    pub fn extension(&self) -> Option<String> {
        let name = self.name?;
        let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
        let (_, ext) = file.rsplit_once('.')?;
        (!ext.is_empty()).then(|| ext.to_ascii_lowercase())
    }

    /// Whether the data starts with `magic`.
    pub fn starts_with(&self, magic: &[u8]) -> bool {
        self.data.starts_with(magic)
    }

    /// Whether the data looks like a zip archive.
    pub fn is_zip(&self) -> bool {
        self.starts_with(b"PK\x03\x04") || self.starts_with(b"PK\x05\x06")
    }

    /// Whether the data looks like an OLE compound file.
    pub fn is_cfb(&self) -> bool {
        self.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])
    }

    /// Entry names of a zip archive (empty for anything else).
    pub fn zip_entries(&self) -> &[String] {
        self.zip_names.get_or_init(|| {
            if !self.is_zip() {
                return Vec::new();
            }
            Zip::new(self.data, DETECTION_READ_LIMIT)
                .map(|z| z.names().to_vec())
                .unwrap_or_default()
        })
    }

    /// Whether the zip archive has an entry (case-insensitive, leading `/`
    /// ignored).
    pub fn zip_contains(&self, name: &str) -> bool {
        let wanted = name.trim_start_matches('/');
        self.zip_entries()
            .iter()
            .any(|n| n.trim_start_matches('/').eq_ignore_ascii_case(wanted))
    }

    /// Read a small zip entry (at most 4 MiB), for example
    /// `[Content_Types].xml` or `mimetype`.
    pub fn zip_read(&self, name: &str) -> Option<Vec<u8>> {
        if !self.is_zip() {
            return None;
        }
        Zip::new(self.data, DETECTION_READ_LIMIT)
            .ok()?
            .read(name)
            .ok()
            .flatten()
    }

    /// Names of the entries in the root storage of a compound file (empty for
    /// anything else).
    pub fn cfb_root_entries(&self) -> &[String] {
        self.cfb_names.get_or_init(|| {
            if !self.is_cfb() {
                return Vec::new();
            }
            Cfb::new(self.data, DETECTION_READ_LIMIT)
                .map(|c| c.list("/").into_iter().map(|e| e.name).collect())
                .unwrap_or_default()
        })
    }

    /// Whether the compound file's root storage has an entry with this name
    /// (case-insensitive).
    pub fn cfb_has(&self, name: &str) -> bool {
        self.cfb_root_entries()
            .iter()
            .any(|n| n.eq_ignore_ascii_case(name))
    }
}
