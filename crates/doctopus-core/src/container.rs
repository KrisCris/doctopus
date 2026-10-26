//! Safe, size-limited access to the two containers behind most office files:
//! zip archives (Office Open XML, OpenDocument, EPUB, ...) and OLE compound
//! files (legacy Office, Outlook messages, embedded OLE objects).

use std::collections::HashMap;
use std::io::{self, Cursor, Read};

/// Read-only view of a zip archive held in memory.
///
/// Entry lookup is case-insensitive and ignores a leading `/`, matching the
/// part-name rules of the Open Packaging Conventions.
///
/// Archives whose central directory is missing (typically truncated
/// downloads) are recovered by scanning the local file headers; whatever
/// entries precede the damage stay readable, and [`Zip::is_recovered`]
/// says so.
pub struct Zip<'a> {
    backend: Backend<'a>,
    names: Vec<String>,
    /// Normalised name → (stored name, entry index).
    index: HashMap<String, (String, usize)>,
    max_entry_size: u64,
}

enum Backend<'a> {
    Archive(zip::ZipArchive<Cursor<&'a [u8]>>),
    /// Entries decompressed while scanning a damaged archive. Entries that
    /// could not be read hold the reason.
    Recovered(Vec<Result<Vec<u8>, String>>),
}

impl std::fmt::Debug for Zip<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Zip")
            .field("entries", &self.names.len())
            .field("recovered", &self.is_recovered())
            .finish()
    }
}

impl<'a> Zip<'a> {
    /// Open an archive. Entries larger than `max_entry_size` bytes
    /// (uncompressed) are refused when read, which defuses zip bombs.
    pub fn new(data: &'a [u8], max_entry_size: u64) -> io::Result<Self> {
        match zip::ZipArchive::new(Cursor::new(data)) {
            Ok(archive) => {
                let entries: Vec<(usize, String)> = archive
                    .file_names()
                    .enumerate()
                    .filter_map(|(i, name)| name.ok().map(|n| (i, n.into_owned())))
                    .collect();
                Ok(Self::with_entries(
                    Backend::Archive(archive),
                    entries,
                    max_entry_size,
                ))
            }
            Err(err) => {
                let (entries, data) = recover(data, max_entry_size);
                if entries.is_empty() {
                    return Err(io::Error::other(err));
                }
                let entries = entries.into_iter().enumerate().collect();
                Ok(Self::with_entries(
                    Backend::Recovered(data),
                    entries,
                    max_entry_size,
                ))
            }
        }
    }

    fn with_entries(
        backend: Backend<'a>,
        entries: Vec<(usize, String)>,
        max_entry_size: u64,
    ) -> Self {
        let names = entries.iter().map(|(_, n)| n.clone()).collect();
        let index = entries
            .into_iter()
            .map(|(i, n)| (normalize_zip_name(&n), (n, i)))
            .collect();
        Self {
            backend,
            names,
            index,
            max_entry_size,
        }
    }

    /// Whether the archive was damaged and its entries were recovered from
    /// local headers (some entries may be missing).
    pub fn is_recovered(&self) -> bool {
        matches!(self.backend, Backend::Recovered(_))
    }

    /// Entry names exactly as stored, in archive order.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// The stored name of an entry, looked up case-insensitively.
    pub fn find(&self, name: &str) -> Option<&str> {
        self.index
            .get(&normalize_zip_name(name))
            .map(|(stored, _)| stored.as_str())
    }

    /// Whether an entry exists.
    pub fn contains(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// Uncompressed size of an entry, as declared by the archive.
    pub fn size(&mut self, name: &str) -> Option<u64> {
        let (_, i) = *self.index.get(&normalize_zip_name(name))?;
        match &mut self.backend {
            Backend::Archive(archive) => archive.by_index(i).ok().map(|f| f.size()),
            Backend::Recovered(data) => data.get(i)?.as_ref().ok().map(|d| d.len() as u64),
        }
    }

    /// Read an entry. Returns `Ok(None)` when it does not exist.
    pub fn read(&mut self, name: &str) -> io::Result<Option<Vec<u8>>> {
        let Some((stored, i)) = self.index.get(&normalize_zip_name(name)).cloned() else {
            return Ok(None);
        };
        let max = self.max_entry_size;
        let archive = match &mut self.backend {
            Backend::Archive(archive) => archive,
            Backend::Recovered(data) => {
                return match data.get(i) {
                    Some(Ok(bytes)) => Ok(Some(bytes.clone())),
                    Some(Err(reason)) => {
                        Err(io::Error::new(io::ErrorKind::InvalidData, reason.clone()))
                    }
                    None => Ok(None),
                };
            }
        };
        let mut file = archive.by_index(i).map_err(io::Error::other)?;
        if file.is_dir() {
            return Ok(None);
        }
        if file.size() > max {
            return Err(too_large(&stored, max));
        }
        let mut buf = Vec::with_capacity(file.size().min(64 << 20) as usize);
        (&mut file).take(max + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > max {
            return Err(too_large(&stored, max));
        }
        Ok(Some(buf))
    }

    /// Read an entry as UTF-8 text (lossy). Returns `Ok(None)` when missing.
    pub fn read_string(&mut self, name: &str) -> io::Result<Option<String>> {
        Ok(self
            .read(name)?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }
}

/// Scan the local file headers of an archive without a usable central
/// directory. Decompressed data is limited to `max_total` bytes overall.
fn recover(data: &[u8], max_total: u64) -> (Vec<String>, Vec<Result<Vec<u8>, String>>) {
    let mut names = Vec::new();
    let mut contents = Vec::new();
    let mut reader = Cursor::new(data);
    let mut total = 0u64;
    while let Ok(Some(mut file)) = zip::read::read_zipfile_from_stream(&mut reader) {
        let name = file.name().map(|n| n.into_owned()).unwrap_or_default();
        if file.is_dir() {
            continue;
        }
        let budget = max_total.saturating_sub(total);
        let mut buf = Vec::new();
        let result = match (&mut file).take(budget + 1).read_to_end(&mut buf) {
            Ok(_) if buf.len() as u64 > budget => Err(format!("`{name}` exceeds the size limit")),
            Ok(_) => Ok(buf),
            // Truncated entry: keep what decompressed, it is the best there is.
            Err(_) if !buf.is_empty() => Ok(buf),
            Err(e) => Err(format!("`{name}` is damaged: {e}")),
        };
        if let Ok(bytes) = &result {
            total += bytes.len() as u64;
        }
        let truncated = result.is_err();
        names.push(name);
        contents.push(result);
        if truncated {
            break;
        }
        // Drain what is left of the entry so the next header is found.
        if io::copy(&mut file, &mut io::sink()).is_err() {
            break;
        }
    }
    (names, contents)
}

fn normalize_zip_name(name: &str) -> String {
    name.trim_start_matches('/')
        .replace('\\', "/")
        .to_lowercase()
}

fn too_large(name: &str, max: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("`{name}` exceeds the {} MiB size limit", max >> 20),
    )
}

/// One entry of a compound file directory.
#[derive(Debug, Clone)]
pub struct CfbEntry {
    /// Entry name (may start with control characters such as `\u{1}`).
    pub name: String,
    /// Full path, `/` separated, starting with `/`.
    pub path: String,
    /// Streams hold bytes; storages hold other entries.
    pub is_stream: bool,
    /// Stream length in bytes (0 for storages).
    pub len: u64,
    /// Class id of a storage, identifying the application that owns it.
    pub clsid: [u8; 16],
}

/// Read-only view of an OLE compound file held in memory.
pub struct Cfb<'a> {
    file: cfb::CompoundFile<Cursor<&'a [u8]>>,
    max_stream_size: u64,
}

impl std::fmt::Debug for Cfb<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cfb").finish_non_exhaustive()
    }
}

impl<'a> Cfb<'a> {
    /// Open a compound file. Streams larger than `max_stream_size` bytes are
    /// refused when read.
    pub fn new(data: &'a [u8], max_stream_size: u64) -> io::Result<Self> {
        let file = cfb::CompoundFile::open(Cursor::new(data))?;
        Ok(Self {
            file,
            max_stream_size,
        })
    }

    /// Whether a stream exists at `path` (case-insensitive, `/` separated).
    pub fn is_stream(&self, path: &str) -> bool {
        self.file.is_stream(path)
    }

    /// Whether a storage exists at `path`.
    pub fn is_storage(&self, path: &str) -> bool {
        self.file.is_storage(path)
    }

    /// Read a whole stream. Returns `Ok(None)` when it does not exist.
    pub fn read(&mut self, path: &str) -> io::Result<Option<Vec<u8>>> {
        if !self.file.is_stream(path) {
            return Ok(None);
        }
        let len = self.file.entry(path)?.len();
        if len > self.max_stream_size {
            return Err(too_large(path, self.max_stream_size));
        }
        let mut stream = self.file.open_stream(path)?;
        let mut buf = Vec::with_capacity(len as usize);
        stream.read_to_end(&mut buf)?;
        Ok(Some(buf))
    }

    /// Entries directly inside a storage (`"/"` for the root).
    pub fn list(&self, storage: &str) -> Vec<CfbEntry> {
        match self.file.read_storage(storage) {
            Ok(entries) => entries.map(|e| to_entry(&e)).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Every entry below the root, depth first.
    pub fn walk(&self) -> Vec<CfbEntry> {
        self.file
            .walk()
            .filter(|e| !e.is_root())
            .map(|e| to_entry(&e))
            .collect()
    }

    /// Class id of the root storage.
    pub fn root_clsid(&self) -> [u8; 16] {
        *self.file.root_entry().clsid().as_bytes()
    }

    /// Copy a storage and everything below it into a new, standalone
    /// compound file. This is how an embedded OLE object (for example
    /// `/ObjectPool/_1234` in a `.doc`) becomes a file of its own.
    pub fn extract_storage(&mut self, path: &str) -> io::Result<Vec<u8>> {
        let base = path.trim_end_matches('/').to_string();
        let entries: Vec<CfbEntry> = {
            let walk = self.file.walk_storage(&base)?;
            walk.map(|e| to_entry(&e)).collect()
        };
        let mut out = cfb::CompoundFile::create(Cursor::new(Vec::new()))?;
        let mut total = 0u64;
        for entry in &entries {
            let Some(relative) = entry.path.strip_prefix(&base) else {
                continue;
            };
            if relative.is_empty() || relative == "/" {
                out.set_storage_clsid("/", uuid::Uuid::from_bytes(entry.clsid))?;
                continue;
            }
            if entry.is_stream {
                total += entry.len;
                if total > self.max_stream_size {
                    return Err(too_large(path, self.max_stream_size));
                }
                let data = self.read(&entry.path)?.unwrap_or_default();
                let mut stream = out.create_stream(relative)?;
                io::Write::write_all(&mut stream, &data)?;
            } else {
                out.create_storage(relative)?;
                out.set_storage_clsid(relative, uuid::Uuid::from_bytes(entry.clsid))?;
            }
        }
        out.flush()?;
        Ok(out.into_inner().into_inner())
    }
}

/// Write a new compound file holding `streams`, each a (path, bytes) pair
/// with the path directly in the root (`/Workbook`). It is a version 3
/// file, which every reader accepts.
pub fn compound_file(streams: &[(&str, &[u8])]) -> io::Result<Vec<u8>> {
    let mut out =
        cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new()))?;
    for (path, data) in streams {
        let mut stream = out.create_stream(path)?;
        io::Write::write_all(&mut stream, data)?;
    }
    out.flush()?;
    Ok(out.into_inner().into_inner())
}

/// Copy a zip archive with the entries named in `replacements` replaced.
/// Every other entry is copied as it is, without recompressing it.
pub fn replace_in_zip(data: &[u8], replacements: &[(&str, &[u8])]) -> io::Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(data)).map_err(io::Error::other)?;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i).map_err(io::Error::other)?;
        let name = entry.name().map_err(io::Error::other)?.into_owned();
        match replacements.iter().find(|(replaced, _)| *replaced == name) {
            Some((name, replacement)) => {
                drop(entry);
                writer.start_file(*name, stored).map_err(io::Error::other)?;
                io::Write::write_all(&mut writer, replacement)?;
            }
            None => writer.raw_copy_file(entry).map_err(io::Error::other)?,
        }
    }
    Ok(writer.finish().map_err(io::Error::other)?.into_inner())
}

fn to_entry(e: &cfb::Entry) -> CfbEntry {
    CfbEntry {
        name: e.name().to_string(),
        path: e.path().to_string_lossy().replace('\\', "/"),
        is_stream: e.is_stream(),
        len: e.len(),
        clsid: *e.clsid().as_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(&mut out);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(data).unwrap();
        }
        writer.finish().unwrap();
        out.into_inner()
    }

    #[test]
    fn zip_lookup_is_case_insensitive_and_size_limited() {
        let data = build_zip(&[("Word/Document.xml", b"<w/>"), ("big.bin", &[0u8; 64])]);
        let mut zip = Zip::new(&data, 32).unwrap();
        assert!(zip.contains("/word/document.xml"));
        assert_eq!(zip.read("word/document.xml").unwrap().unwrap(), b"<w/>");
        assert!(zip.read("missing").unwrap().is_none());
        assert!(zip.read("big.bin").is_err());
    }

    #[test]
    fn truncated_zips_are_recovered_from_local_headers() {
        let data = build_zip(&[("a.txt", b"first"), ("b.txt", b"second")]);
        // Cut off the central directory.
        let cut = data.len() - 120;
        let mut zip = Zip::new(&data[..cut], 1 << 20).unwrap();
        assert!(zip.is_recovered());
        assert_eq!(zip.read("a.txt").unwrap().unwrap(), b"first");
        assert_eq!(zip.read("b.txt").unwrap().unwrap(), b"second");
        assert!(Zip::new(b"PK\x03\x04garbage", 1 << 20).is_err());
    }

    #[test]
    fn zip_entries_are_replaced_and_the_rest_copied() {
        let data = build_zip(&[("a.txt", b"first"), ("b.txt", b"second")]);
        let copy = replace_in_zip(&data, &[("b.txt", b"changed")]).unwrap();
        let mut zip = Zip::new(&copy, 1 << 20).unwrap();
        assert!(!zip.is_recovered());
        assert_eq!(zip.names(), ["a.txt", "b.txt"]);
        assert_eq!(zip.read("a.txt").unwrap().unwrap(), b"first");
        assert_eq!(zip.read("b.txt").unwrap().unwrap(), b"changed");
    }

    #[test]
    fn cfb_storage_round_trips() {
        let mut file = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        file.create_storage("/Pool").unwrap();
        file.create_storage("/Pool/_1").unwrap();
        file.create_stream("/Pool/_1/CONTENTS")
            .unwrap()
            .write_all(b"%PDF-1.4")
            .unwrap();
        file.flush().unwrap();
        let data = file.into_inner().into_inner();

        let mut cfb = Cfb::new(&data, 1 << 20).unwrap();
        assert!(cfb.is_storage("/Pool/_1"));
        let extracted = cfb.extract_storage("/Pool/_1").unwrap();
        let mut inner = Cfb::new(&extracted, 1 << 20).unwrap();
        assert_eq!(inner.read("/CONTENTS").unwrap().unwrap(), b"%PDF-1.4");
    }
}
