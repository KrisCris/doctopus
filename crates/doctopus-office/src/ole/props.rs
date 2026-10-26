//! OLE property sets (MS-OLEPS): the `\x05SummaryInformation` and
//! `\x05DocumentSummaryInformation` streams that carry the title, author,
//! dates and custom properties of 97-2003 files.

use std::collections::HashMap;

use doctopus_core::Document;
use doctopus_core::container::Cfb;

use super::text::{decode_codepage, utf16le};
use crate::binary::Cursor;
use crate::dates::filetime;

const SUMMARY: [u8; 16] = fmtid(
    0xF29F_85E0,
    0x4FF9,
    0x1068,
    [0xAB, 0x91, 0x08, 0x00, 0x2B, 0x27, 0xB3, 0xD9],
);
const DOC_SUMMARY: [u8; 16] = fmtid(
    0xD5CD_D502,
    0x2E9C,
    0x101B,
    [0x93, 0x97, 0x08, 0x00, 0x2B, 0x2C, 0xF9, 0xAE],
);
const USER_DEFINED: [u8; 16] = fmtid(
    0xD5CD_D505,
    0x2E9C,
    0x101B,
    [0x93, 0x97, 0x08, 0x00, 0x2B, 0x2C, 0xF9, 0xAE],
);

/// The UTF-16 "code page" of property sets written by Unicode applications.
const CP_UTF16: u16 = 1200;

/// Properties per set read at most; real files have a few dozen.
const MAX_PROPERTIES: u32 = 4096;

/// A GUID in its little-endian byte layout.
const fn fmtid(a: u32, b: u16, c: u16, d: [u8; 8]) -> [u8; 16] {
    let a = a.to_le_bytes();
    let b = b.to_le_bytes();
    let c = c.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d[0], d[1], d[2], d[3], d[4], d[5], d[6],
        d[7],
    ]
}

/// A property value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Prop {
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// A FILETIME: a point in time or, for editing time, a duration.
    Time(u64),
    Texts(Vec<String>),
}

/// One property set: values by property id and, for user-defined
/// properties, their names.
#[derive(Debug, Default)]
pub(crate) struct PropertySet {
    pub(crate) values: Vec<(u32, Prop)>,
    pub(crate) names: HashMap<u32, String>,
}

impl PropertySet {
    fn get(&self, id: u32) -> Option<&Prop> {
        self.values.iter().find(|(i, _)| *i == id).map(|(_, v)| v)
    }
}

/// Read both summary streams of a compound file into `doc.metadata`.
pub(crate) fn read(cfb: &mut Cfb<'_>, doc: &mut Document) {
    if let Ok(Some(data)) = cfb.read("/\u{5}SummaryInformation") {
        for (id, set) in parse(&data) {
            if id == SUMMARY {
                summary(&set, doc);
            }
        }
    }
    if let Ok(Some(data)) = cfb.read("/\u{5}DocumentSummaryInformation") {
        for (id, set) in parse(&data) {
            match id {
                DOC_SUMMARY => doc_summary(&set, doc),
                USER_DEFINED => user_defined(&set, doc),
                _ => {}
            }
        }
    }
}

fn summary(set: &PropertySet, doc: &mut Document) {
    for (id, value) in &set.values {
        let key = match id {
            2 => "title",
            3 => "subject",
            4 => "author",
            5 => "keywords",
            6 => "description",
            7 => "template",
            8 => "last_modified_by",
            9 => "revision",
            12 => "created",
            13 => "modified",
            14 => "pages",
            15 => "words",
            16 => "characters",
            18 => "application",
            10 => {
                // Editing time is a duration stored as a FILETIME.
                if let Prop::Time(t) = value {
                    let minutes = t / 600_000_000;
                    if minutes > 0 {
                        doc.metadata
                            .set("total_editing_time", format!("{minutes} minutes"));
                    }
                }
                continue;
            }
            _ => continue,
        };
        let text = match value {
            Prop::Time(t) => match filetime(*t) {
                Some(t) => t,
                None => continue,
            },
            // Statistics of zero are placeholders written by generators.
            Prop::Int(0) if matches!(key, "pages" | "words" | "characters") => continue,
            other => display(other),
        };
        if !text.trim().is_empty() {
            doc.metadata.set(key, text.trim());
        }
    }
}

fn doc_summary(set: &PropertySet, doc: &mut Document) {
    for (id, key) in [(2, "category"), (14, "manager"), (15, "company")] {
        if let Some(value) = set.get(id).map(display).filter(|v| !v.trim().is_empty()) {
            doc.metadata.set(key, value.trim());
        }
    }
}

fn user_defined(set: &PropertySet, doc: &mut Document) {
    for (id, value) in &set.values {
        let Some(name) = set.names.get(id) else {
            continue;
        };
        if is_bookkeeping(name) {
            continue;
        }
        let text = match value {
            Prop::Time(t) => filetime(*t).unwrap_or_default(),
            other => display(other),
        };
        doc.metadata.set(&format!("custom.{name}"), text);
    }
}

/// Custom properties that applications keep for themselves rather than
/// for the reader: document-management and labelling ids, Visio build
/// numbers, copies of application statistics some converters write, and
/// WPS Office markers.
pub(crate) fn is_bookkeeping(name: &str) -> bool {
    const NAMES: [&str; 11] = [
        "ContentTypeId",
        "BuildNumberCreated",
        "BuildNumberEdited",
        "IsMetric",
        "TimeEdited",
        "AppVersion",
        "DocSecurity",
        "HyperlinksChanged",
        "LinksUpToDate",
        "ScaleCrop",
        "ShareDoc",
    ];
    name.starts_with('_')
        || name.starts_with("MSIP_Label")
        || name.starts_with("KSO")
        || name == "ICV"
        || NAMES.contains(&name)
}

fn display(value: &Prop) -> String {
    match value {
        Prop::Text(s) => s.clone(),
        Prop::Int(n) => n.to_string(),
        Prop::Float(f) => doctopus_core::format_number(*f),
        Prop::Bool(b) => b.to_string(),
        Prop::Time(t) => filetime(*t).unwrap_or_default(),
        Prop::Texts(v) => v.join(", "),
    }
}

/// Parse a property set stream into its sets, by format id. Damaged sets
/// yield whatever was read before the damage.
pub(crate) fn parse(data: &[u8]) -> Vec<([u8; 16], PropertySet)> {
    let mut r = Cursor::new(data);
    let header = (|| {
        if r.u16()? != 0xFFFE {
            return None;
        }
        r.skip(2 + 4 + 16)?;
        let count = r.u32()?.min(16);
        let mut sets = Vec::new();
        for _ in 0..count {
            let id: [u8; 16] = r.take(16)?.try_into().ok()?;
            sets.push((id, r.u32()? as usize));
        }
        Some(sets)
    })();
    header
        .unwrap_or_default()
        .into_iter()
        .map(|(id, offset)| (id, parse_set(data, offset)))
        .collect()
}

fn parse_set(data: &[u8], start: usize) -> PropertySet {
    let mut set = PropertySet::default();
    let mut r = Cursor::at(data, start);
    let Some((_size, count)) = r.u32().zip(r.u32()) else {
        return set;
    };
    let mut entries = Vec::new();
    for _ in 0..count.min(MAX_PROPERTIES) {
        let Some((id, offset)) = r.u32().zip(r.u32()) else {
            break;
        };
        entries.push((id, start.saturating_add(offset as usize)));
    }
    // The code page (property 1) decides how 8-bit strings are decoded.
    let codepage = entries
        .iter()
        .find(|(id, _)| *id == 1)
        .and_then(|(_, at)| match value(data, *at, 1252)? {
            Prop::Int(cp) => Some(cp as u16),
            _ => None,
        })
        .unwrap_or(1252);
    for (id, at) in entries {
        match id {
            0 => set.names = dictionary(data, at, codepage).unwrap_or_default(),
            1 => {}
            _ => {
                if let Some(v) = value(data, at, codepage) {
                    set.values.push((id, v));
                }
            }
        }
    }
    set
}

fn value(data: &[u8], at: usize, codepage: u16) -> Option<Prop> {
    let mut r = Cursor::at(data, at);
    let vt = r.u16()?;
    r.skip(2)?;
    typed(&mut r, vt, codepage)
}

fn typed(r: &mut Cursor<'_>, vt: u16, codepage: u16) -> Option<Prop> {
    Some(match vt {
        // VT_I2, VT_UI2
        2 | 18 => Prop::Int(i64::from(r.i16()?)),
        // VT_I4, VT_INT, VT_UI4, VT_UINT, VT_ERROR
        3 | 22 => Prop::Int(i64::from(r.i32()?)),
        19 | 23 => Prop::Int(i64::from(r.u32()?)),
        // VT_I8, VT_UI8
        20 | 21 => Prop::Int(r.u64()? as i64),
        // VT_R4, VT_R8
        4 => Prop::Float(f64::from(f32::from_bits(r.u32()?))),
        5 => Prop::Float(r.f64()?),
        // VT_BOOL
        11 => Prop::Bool(r.u16()? != 0),
        // VT_LPSTR, VT_BSTR
        30 | 8 => Prop::Text(code_page_string(r, codepage)?),
        // VT_LPWSTR
        31 => {
            let chars = r.u32()? as usize;
            let raw = r.take(chars.checked_mul(2)?)?;
            r.skip((4 - raw.len() % 4) % 4);
            Prop::Text(utf16le(raw))
        }
        // VT_FILETIME
        64 => Prop::Time(r.u64()?),
        // VT_VECTOR of strings or variants (titles of parts, heading pairs)
        0x101E | 0x101F | 0x100C => {
            let count = r.u32()?.min(1024);
            let mut items = Vec::new();
            for _ in 0..count {
                let item_vt = if vt == 0x100C {
                    let t = r.u16()?;
                    r.skip(2)?;
                    t
                } else {
                    vt & 0xFF
                };
                match typed(r, item_vt, codepage)? {
                    Prop::Text(s) => items.push(s),
                    Prop::Int(n) => items.push(n.to_string()),
                    _ => {}
                }
            }
            Prop::Texts(items)
        }
        _ => return None,
    })
}

/// A string with a 32-bit byte length, in the property set's code page.
fn code_page_string(r: &mut Cursor<'_>, codepage: u16) -> Option<String> {
    let len = r.u32()? as usize;
    if len > 0x10_0000 {
        return None;
    }
    let raw = r.take(len)?;
    r.skip((4 - len % 4) % 4);
    Some(if codepage == CP_UTF16 {
        utf16le(raw)
    } else {
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        decode_codepage(&raw[..end], codepage)
    })
}

/// Property 0: names of user-defined properties.
fn dictionary(data: &[u8], at: usize, codepage: u16) -> Option<HashMap<u32, String>> {
    let mut r = Cursor::at(data, at);
    let count = r.u32()?.min(MAX_PROPERTIES);
    let mut out = HashMap::new();
    for _ in 0..count {
        let id = r.u32()?;
        let len = r.u32()? as usize;
        let name = if codepage == CP_UTF16 {
            let raw = r.take(len.checked_mul(2)?)?;
            r.skip((4 - raw.len() % 4) % 4);
            utf16le(raw)
        } else {
            let raw = r.take(len)?;
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            decode_codepage(&raw[..end], codepage)
        };
        out.insert(id, name);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A property set stream with one set holding the given properties.
    fn stream(fmt: [u8; 16], props: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut set = Vec::new();
        let header_len = 8 + 8 * props.len();
        let mut body = Vec::new();
        let mut index = Vec::new();
        for (id, value) in props {
            index.extend_from_slice(&id.to_le_bytes());
            index.extend_from_slice(&((header_len + body.len()) as u32).to_le_bytes());
            body.extend_from_slice(value);
        }
        set.extend_from_slice(&((header_len + body.len()) as u32).to_le_bytes());
        set.extend_from_slice(&(props.len() as u32).to_le_bytes());
        set.extend(index);
        set.extend(body);
        let mut out = vec![0xFE, 0xFF, 0, 0, 0, 0, 0, 0];
        out.extend_from_slice(&[0; 16]);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(&48u32.to_le_bytes());
        out.extend(set);
        out
    }

    fn lpstr(s: &str) -> Vec<u8> {
        let mut v = vec![30, 0, 0, 0];
        let len = s.len() + 1;
        v.extend_from_slice(&(len as u32).to_le_bytes());
        v.extend_from_slice(s.as_bytes());
        v.push(0);
        while v.len() % 4 != 0 {
            v.push(0);
        }
        v
    }

    #[test]
    fn reads_summary_properties() {
        let mut created = vec![64, 0, 0, 0];
        created.extend_from_slice(&128_439_210_960_000_000u64.to_le_bytes());
        let codepage = vec![2, 0, 0, 0, 0xE4, 0x04, 0, 0];
        let data = stream(
            SUMMARY,
            &[
                (1, codepage),
                (2, lpstr("Plan")),
                (4, lpstr("Ana")),
                (12, created),
            ],
        );
        let sets = parse(&data);
        assert_eq!(sets.len(), 1);
        let mut doc = Document::new(&crate::formats::DOC);
        summary(&sets[0].1, &mut doc);
        assert_eq!(doc.metadata.title.as_deref(), Some("Plan"));
        assert_eq!(doc.metadata.author.as_deref(), Some("Ana"));
        assert_eq!(
            doc.metadata.created.as_deref(),
            Some("2008-01-04T11:51:36Z")
        );
    }

    #[test]
    fn user_defined_properties_are_named_by_the_dictionary() {
        let mut dict = Vec::new();
        dict.extend_from_slice(&1u32.to_le_bytes());
        dict.extend_from_slice(&2u32.to_le_bytes());
        dict.extend_from_slice(&7u32.to_le_bytes());
        dict.extend_from_slice(b"Client\0");
        dict.push(0);
        let data = stream(USER_DEFINED, &[(0, dict), (2, lpstr("ACME"))]);
        let sets = parse(&data);
        let mut doc = Document::new(&crate::formats::DOC);
        user_defined(&sets[0].1, &mut doc);
        assert_eq!(
            doc.metadata.extra.get("custom.Client").map(String::as_str),
            Some("ACME")
        );
    }

    #[test]
    fn garbage_is_ignored() {
        assert!(parse(&[0xFE, 0xFF, 1]).is_empty());
        assert!(parse(b"not a property set").is_empty());
    }
}
