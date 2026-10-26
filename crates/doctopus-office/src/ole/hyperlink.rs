//! Hyperlink objects (MS-OSHARED 2.3.7.1): the binary hyperlinks of Excel's
//! `HLINK` records and of shapes in OfficeArt drawings.

use super::text::{ansi, utf16le};
use crate::binary::Cursor;

/// `{79EAC9D0-BAF9-11CE-8C82-00AA004BA90B}`: StdHlink, written before the
/// object when it is stored as a COM object.
const STD_HLINK: [u8; 16] = [
    0xD0, 0xC9, 0xEA, 0x79, 0xF9, 0xBA, 0xCE, 0x11, 0x8C, 0x82, 0x00, 0xAA, 0x00, 0x4B, 0xA9, 0x0B,
];
/// `{79EAC9E0-BAF9-11CE-8C82-00AA004BA90B}`: URL moniker.
const URL_MONIKER: [u8; 16] = [
    0xE0, 0xC9, 0xEA, 0x79, 0xF9, 0xBA, 0xCE, 0x11, 0x8C, 0x82, 0x00, 0xAA, 0x00, 0x4B, 0xA9, 0x0B,
];
/// `{00000303-0000-0000-C000-000000000046}`: file moniker.
const FILE_MONIKER: [u8; 16] = [0x03, 0x03, 0, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46];

const HAS_MONIKER: u32 = 0x01;
const HAS_LOCATION: u32 = 0x08;
const HAS_DISPLAY_NAME: u32 = 0x10;
const HAS_FRAME_NAME: u32 = 0x80;
const MONIKER_SAVED_AS_STRING: u32 = 0x100;

/// The target of a hyperlink object: a URL or file path, with `#location`
/// for links into a document; `#location` alone for links within it.
pub(crate) fn parse(data: &[u8]) -> Option<String> {
    let mut r = Cursor::new(data);
    if data.starts_with(&STD_HLINK) {
        r.skip(16)?;
    }
    if r.u32()? != 2 {
        return None;
    }
    let flags = r.u32()?;
    if flags & HAS_DISPLAY_NAME != 0 {
        string(&mut r)?;
    }
    if flags & HAS_FRAME_NAME != 0 {
        string(&mut r)?;
    }
    let mut target = None;
    if flags & HAS_MONIKER != 0 {
        target = if flags & MONIKER_SAVED_AS_STRING != 0 {
            Some(string(&mut r)?)
        } else {
            moniker(&mut r)
        };
    }
    let location = if flags & HAS_LOCATION != 0 {
        string(&mut r)
    } else {
        None
    };
    let target = target
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let location = location
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty());
    match (target, location) {
        (Some(t), Some(l)) => Some(format!("{t}#{l}")),
        (Some(t), None) => Some(t),
        (None, Some(l)) => Some(format!("#{l}")),
        (None, None) => None,
    }
}

/// A HyperlinkString: length in characters (NUL included), then UTF-16.
fn string(r: &mut Cursor<'_>) -> Option<String> {
    let chars = r.u32()? as usize;
    if chars > 0x10_0000 {
        return None;
    }
    Some(utf16le(r.take(chars * 2)?))
}

fn moniker(r: &mut Cursor<'_>) -> Option<String> {
    let clsid = r.take(16)?;
    if clsid == URL_MONIKER {
        let len = r.u32()? as usize;
        let body = r.take(len.min(r.remaining()))?;
        Some(utf16le(body))
    } else if clsid == FILE_MONIKER {
        let up_levels = r.u16()?;
        let ansi_len = r.u32()? as usize;
        let ansi_path = r.take(ansi_len)?;
        let ansi_path = ansi(
            &ansi_path[..ansi_path
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(ansi_path.len())],
        );
        r.skip(2 + 2 + 16 + 4)?;
        let unicode_size = r.u32().unwrap_or(0);
        let path = if unicode_size > 0 {
            let bytes = r.u32()? as usize;
            r.skip(2)?;
            utf16le(r.take(bytes)?)
        } else {
            ansi_path
        };
        Some(format!("{}{path}", "..\\".repeat(usize::from(up_levels))))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hstring(s: &str) -> Vec<u8> {
        let units: Vec<u16> = s.encode_utf16().chain([0]).collect();
        let mut out = (units.len() as u32).to_le_bytes().to_vec();
        out.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        out
    }

    #[test]
    fn url_monikers_and_locations() {
        let mut data = 2u32.to_le_bytes().to_vec();
        data.extend_from_slice(&(HAS_MONIKER | HAS_DISPLAY_NAME | HAS_LOCATION).to_le_bytes());
        data.extend(hstring("Docs"));
        data.extend_from_slice(&URL_MONIKER);
        let url: Vec<u8> = "https://example.com/\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        data.extend_from_slice(&(url.len() as u32).to_le_bytes());
        data.extend(url);
        data.extend(hstring("intro"));
        assert_eq!(parse(&data).as_deref(), Some("https://example.com/#intro"));
    }

    #[test]
    fn locations_alone_point_inside_the_document() {
        let mut data = STD_HLINK.to_vec();
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&HAS_LOCATION.to_le_bytes());
        data.extend(hstring("Sheet2!A1"));
        assert_eq!(parse(&data).as_deref(), Some("#Sheet2!A1"));
    }

    #[test]
    fn file_monikers() {
        let mut data = 2u32.to_le_bytes().to_vec();
        data.extend_from_slice(&HAS_MONIKER.to_le_bytes());
        data.extend_from_slice(&FILE_MONIKER);
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&9u32.to_le_bytes());
        data.extend_from_slice(b"data.txt\0");
        data.extend_from_slice(&[0xFF, 0xFF, 0xAD, 0xDE]);
        data.extend_from_slice(&[0; 20]);
        data.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse(&data).as_deref(), Some("..\\data.txt"));
    }
}
