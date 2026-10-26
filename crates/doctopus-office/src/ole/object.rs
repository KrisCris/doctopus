//! Embedded OLE objects: turning the storage an Office application keeps
//! for an embedded object back into the file the user inserted.
//!
//! An object embedded in an Office file is an OLE compound file (a stream
//! of `oleObject1.bin`, or a storage under `ObjectPool`). Depending on the
//! application that created it, the actual payload is
//!
//! * the compound file itself (Word, Excel, PowerPoint and Visio 97-2003
//!   documents keep their streams at the root),
//! * an `\x01Ole10Native` stream (files inserted with the Packager, "Create
//!   from file"), which also records the original file name,
//! * a `Package` stream (Office Open XML documents embedded in a 97-2003
//!   file),
//! * a `CONTENTS` stream (PDF and some other formats), or
//! * an `EmbeddedOdf` stream (OpenDocument files).

use doctopus_core::container::Cfb;
use doctopus_core::sniff::sniff;
use doctopus_core::{Part, PartKind};

use super::text::utf16le;
use crate::binary::Cursor;

/// What an embedded object turned out to contain.
#[derive(Debug, Clone)]
pub(crate) struct Unwrapped {
    /// The payload: the inserted file when it could be recovered, otherwise
    /// the object storage itself.
    pub(crate) data: Vec<u8>,
    /// The original file name, when the object records one.
    pub(crate) name: Option<String>,
    /// Media type of the payload, when known from the object's structure.
    pub(crate) content_type: Option<&'static str>,
    /// Programmatic id of the creating application (`Excel.Sheet.12`).
    pub(crate) prog_id: Option<String>,
    /// Human-readable type (`Microsoft Excel Worksheet`).
    pub(crate) user_type: Option<String>,
}

impl Unwrapped {
    /// The part holding this object. `fallback_name` names the part when the
    /// object records no file name (generated names such as
    /// `oleObject1.bin` are dropped); `declared_type` is the content type
    /// the container document gives it.
    pub(crate) fn into_part(
        self,
        origin: String,
        location: &str,
        fallback_name: Option<String>,
        declared_type: Option<&str>,
    ) -> Part {
        let kind = if self.name.is_some() {
            PartKind::File
        } else {
            PartKind::Object
        };
        let description = self.description();
        let declared = declared_type
            .filter(|ct| !ct.contains("oleObject") && *ct != "application/octet-stream");
        let name = self.name.or(fallback_name).filter(|n| {
            let lower = n.to_ascii_lowercase();
            !(lower.starts_with("oleobject") && lower.ends_with(".bin"))
        });
        let mut part = Part::new(kind, self.data)
            .with_description(description)
            .with_origin(origin)
            .with_location(location);
        if let Some(ct) = self.content_type.or(declared) {
            part = part.with_content_type(ct);
        }
        if let Some(name) = name {
            part = part.with_name(name);
        }
        part
    }

    /// A one-line description such as `OLE object Excel.Sheet.12
    /// (Microsoft Excel Worksheet)`.
    pub(crate) fn description(&self) -> String {
        if self.prog_id.is_none() && self.user_type.is_none() {
            return "embedded file".into();
        }
        let mut out = String::from("OLE object");
        if let Some(prog_id) = &self.prog_id {
            out.push(' ');
            out.push_str(prog_id);
        }
        match &self.user_type {
            Some(user_type) if self.prog_id.as_deref() != Some(user_type.as_str()) => {
                out.push_str(&format!(" ({user_type})"));
            }
            _ => {}
        }
        out
    }
}

/// Whether `data` is a compound file without a single stream: what Office
/// leaves for an object that was never saved.
pub(crate) fn is_empty_storage(data: &[u8]) -> bool {
    data.starts_with(&CFB_MAGIC)
        && Cfb::new(data, 0).is_ok_and(|cfb| cfb.walk().iter().all(|e| !e.is_stream))
}

/// Unwrap an embedded object. `prog_id` is the application id recorded by
/// the container document, used when the object does not name one itself.
/// Input that is not a compound file is returned unchanged.
pub(crate) fn unwrap(data: Vec<u8>, prog_id: Option<&str>, max_size: u64) -> Unwrapped {
    let fallback = |data: Vec<u8>| Unwrapped {
        data,
        name: None,
        content_type: None,
        prog_id: prog_id.map(str::to_string),
        user_type: None,
    };
    if !data.starts_with(&CFB_MAGIC) {
        return fallback(data);
    }
    let Ok(mut cfb) = Cfb::new(&data, max_size) else {
        return fallback(data);
    };
    let comp_obj = cfb
        .read("/\u{1}CompObj")
        .ok()
        .flatten()
        .and_then(|bytes| CompObj::parse(&bytes));
    let prog_id = comp_obj
        .as_ref()
        .and_then(|c| c.prog_id.clone())
        .or_else(|| prog_id.map(str::to_string))
        .or_else(|| known_class(&cfb.root_clsid()).map(str::to_string));
    let user_type = comp_obj.and_then(|c| c.user_type);

    let mut result = Unwrapped {
        data: Vec::new(),
        name: None,
        content_type: None,
        prog_id,
        user_type,
    };

    if let Some(native) = cfb.read("/\u{1}Ole10Native").ok().flatten()
        && let Some(file) = Ole10Native::parse(&native)
    {
        result.name = file.name;
        result.data = file.data;
        return result;
    }
    for stream in ["/Package", "/EmbeddedOdf"] {
        if let Some(payload) = cfb.read(stream).ok().flatten()
            && payload.starts_with(b"PK\x03\x04")
        {
            result.data = payload;
            return result;
        }
    }
    if let Some(payload) = cfb.read("/CONTENTS").ok().flatten() {
        let recognised = sniff(&payload).is_some_and(|s| !s.mime.starts_with("text/"));
        if recognised {
            result.data = payload;
            return result;
        }
    }
    result.content_type = if cfb.is_stream("/WordDocument") {
        Some("application/msword")
    } else if cfb.is_stream("/Workbook") || cfb.is_stream("/Book") {
        Some("application/vnd.ms-excel")
    } else if cfb.is_stream("/PowerPoint Document") {
        Some("application/vnd.ms-powerpoint")
    } else if cfb.is_stream("/VisioDocument") {
        Some("application/vnd.visio")
    } else {
        None
    };
    drop(cfb);
    result.data = data;
    result
}

const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// Programmatic ids for the class ids of common embedded objects, used
/// when an object carries no `CompObj` stream.
fn known_class(clsid: &[u8; 16]) -> Option<&'static str> {
    let id = format_clsid(clsid);
    Some(match id.as_str() {
        "00020906-0000-0000-C000-000000000046" => "Word.Document.8",
        "F4754C9B-64F5-4B40-8AF4-679732AC0607" => "Word.Document.12",
        "00020820-0000-0000-C000-000000000046" => "Excel.Sheet.8",
        "00020821-0000-0000-C000-000000000046" => "Excel.Chart.8",
        "00020830-0000-0000-C000-000000000046" => "Excel.Sheet.12",
        "64818D10-4F9B-11CF-86EA-00AA00B929E8" => "PowerPoint.Show.8",
        "CF4F55F4-8F87-4D47-80BB-5808164BB3F8" => "PowerPoint.Show.12",
        "0003000C-0000-0000-C000-000000000046" => "Package",
        "0002CE02-0000-0000-C000-000000000046" => "Equation.3",
        "00021A14-0000-0000-C000-000000000046" => "Visio.Drawing.11",
        "B801CA65-A1FC-11D0-85AD-444553540000" => "AcroExch.Document",
        _ => return None,
    })
}

/// Canonical `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX` form of a class id held
/// in big-endian byte order.
pub(crate) fn format_clsid(b: &[u8; 16]) -> String {
    let hex = |range: std::ops::Range<usize>| -> String {
        b[range].iter().map(|x| format!("{x:02X}")).collect()
    };
    format!(
        "{}-{}-{}-{}-{}",
        hex(0..4),
        hex(4..6),
        hex(6..8),
        hex(8..10),
        hex(10..16)
    )
}

/// The `\x01CompObj` stream: which application owns an object.
#[derive(Debug, Default)]
struct CompObj {
    user_type: Option<String>,
    prog_id: Option<String>,
}

impl CompObj {
    /// Layout per [MS-OLEDS] 2.3.8 (CompObjStream).
    fn parse(data: &[u8]) -> Option<Self> {
        let mut r = Cursor::new(data);
        r.skip(28)?;
        let user_type = r.ansi_string()?;
        // Clipboard format: a marker, then either a format number or a name.
        match r.u32()? {
            0 => {}
            0xFFFF_FFFF | 0xFFFF_FFFE => {
                r.u32()?;
            }
            len => {
                r.skip(len as usize)?;
            }
        }
        let prog_id = r.ansi_string();
        // Unicode versions follow in newer files and win when present.
        let mut unicode_user_type = None;
        if r.u32() == Some(0x71B2_39F4) {
            unicode_user_type = r.unicode_string();
        }
        Some(Self {
            user_type: unicode_user_type
                .or(Some(user_type))
                .filter(|s| !s.is_empty()),
            prog_id: prog_id.filter(|s| !s.is_empty()),
        })
    }
}

/// A file inserted with the Packager (`\x01Ole10Native` stream).
#[derive(Debug)]
struct Ole10Native {
    name: Option<String>,
    data: Vec<u8>,
}

impl Ole10Native {
    fn parse(stream: &[u8]) -> Option<Self> {
        let mut r = Cursor::new(stream);
        let total = r.u32()? as usize;
        let body = stream.get(4..)?;
        let body = &body[..total.min(body.len())];
        Self::parse_structured(body).or_else(|| {
            // "Compact" form: the size header is followed by the bare file.
            (!body.is_empty()).then(|| Self {
                name: None,
                data: body.to_vec(),
            })
        })
    }

    fn parse_structured(body: &[u8]) -> Option<Self> {
        let mut r = Cursor::new(body);
        if r.u16()? != 2 {
            return None;
        }
        let label = r.zstring()?;
        let source_path = r.zstring()?;
        r.u16()?;
        r.u16()?;
        let temp_len = r.u32()? as usize;
        r.skip(temp_len)?;
        let data_len = r.u32()? as usize;
        let data = r.take(data_len)?.to_vec();
        // Optional Unicode copies of the temp path, label and source path.
        let mut unicode = Vec::new();
        for _ in 0..3 {
            match r.u32() {
                Some(chars) if chars < 32_768 => match r.take(chars as usize * 2) {
                    Some(raw) => unicode.push(utf16le(raw)),
                    None => break,
                },
                _ => break,
            }
        }
        let label = unicode
            .get(1)
            .cloned()
            .filter(|s| !s.is_empty())
            .unwrap_or(label);
        let source_path = unicode
            .get(2)
            .cloned()
            .filter(|s| !s.is_empty())
            .unwrap_or(source_path);
        let from_path = source_path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        // The label is what the user saw; it normally is the file name, but
        // can be a caption without extension.
        let name = if label.contains('.') || from_path.is_empty() {
            label
        } else {
            from_path
        };
        Some(Self {
            name: (!name.trim().is_empty()).then_some(name),
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor as IoCursor, Write};

    use super::*;

    fn compound_file(streams: &[(&str, &[u8])]) -> Vec<u8> {
        let mut file = cfb::CompoundFile::create(IoCursor::new(Vec::new())).unwrap();
        for (path, data) in streams {
            file.create_stream(path).unwrap().write_all(data).unwrap();
        }
        file.flush().unwrap();
        file.into_inner().into_inner()
    }

    fn ole10native(label: &str, path: &str, payload: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&2u16.to_le_bytes());
        body.extend_from_slice(label.as_bytes());
        body.push(0);
        body.extend_from_slice(path.as_bytes());
        body.push(0);
        body.extend_from_slice(&[0, 0, 3, 0]);
        let temp = b"C:\\Temp\\x\0";
        body.extend_from_slice(&(temp.len() as u32).to_le_bytes());
        body.extend_from_slice(temp);
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(payload);
        let mut stream = (body.len() as u32).to_le_bytes().to_vec();
        stream.extend_from_slice(&body);
        stream
    }

    #[test]
    fn packager_objects_yield_the_inserted_file() {
        let native = ole10native("notes.txt", "C:\\Users\\me\\notes.txt", b"hello");
        let data = compound_file(&[("/\u{1}Ole10Native", &native)]);
        let unwrapped = unwrap(data, Some("Package"), 1 << 20);
        assert_eq!(unwrapped.data, b"hello");
        assert_eq!(unwrapped.name.as_deref(), Some("notes.txt"));
        assert_eq!(unwrapped.prog_id.as_deref(), Some("Package"));
    }

    #[test]
    fn label_without_extension_falls_back_to_the_path() {
        let native = ole10native("My notes", "C:\\docs\\notes.txt", b"hi");
        let file = Ole10Native::parse(&native).unwrap();
        assert_eq!(file.name.as_deref(), Some("notes.txt"));
    }

    #[test]
    fn package_and_contents_streams_are_unwrapped() {
        let zip = b"PK\x03\x04rest-of-a-docx";
        let data = compound_file(&[("/Package", zip)]);
        assert_eq!(unwrap(data, None, 1 << 20).data, zip);
        let pdf = b"%PDF-1.7 ...";
        let data = compound_file(&[("/CONTENTS", pdf)]);
        assert_eq!(unwrap(data, None, 1 << 20).data, pdf);
    }

    #[test]
    fn legacy_documents_stay_whole_and_typed() {
        let data = compound_file(&[("/WordDocument", b"\xec\xa5"), ("/1Table", b"")]);
        let unwrapped = unwrap(data.clone(), Some("Word.Document.8"), 1 << 20);
        assert_eq!(unwrapped.data, data);
        assert_eq!(unwrapped.content_type, Some("application/msword"));
        assert_eq!(unwrapped.description(), "OLE object Word.Document.8");
    }

    #[test]
    fn non_ole_payloads_pass_through() {
        let unwrapped = unwrap(b"PK\x03\x04".to_vec(), Some("Excel.Sheet.12"), 1 << 20);
        assert_eq!(unwrapped.data, b"PK\x03\x04");
        assert_eq!(unwrapped.prog_id.as_deref(), Some("Excel.Sheet.12"));
    }

    #[test]
    fn class_ids_are_formatted_canonically() {
        let bytes = [
            0x00, 0x02, 0x09, 0x06, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
        ];
        assert_eq!(format_clsid(&bytes), "00020906-0000-0000-C000-000000000046");
        assert_eq!(known_class(&bytes), Some("Word.Document.8"));
    }
}
