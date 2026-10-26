//! OfficeArt (MS-ODRAW, "Escher"): the drawing layer shared by the 97-2003
//! Word, Excel and PowerPoint formats. This module reads its record tree,
//! the pictures (BLIPs) of the picture store, and the shape properties that
//! say which picture a shape shows and what it is called.

use std::io::Read;

use crate::binary::Cursor;
use crate::ole::text::utf16le;

/// Record types.
pub(crate) mod rt {
    pub(crate) const DGG_CONTAINER: u16 = 0xF000;
    pub(crate) const BSTORE_CONTAINER: u16 = 0xF001;
    pub(crate) const DG_CONTAINER: u16 = 0xF002;
    pub(crate) const SPGR_CONTAINER: u16 = 0xF003;
    pub(crate) const SP_CONTAINER: u16 = 0xF004;
    pub(crate) const FBSE: u16 = 0xF007;
    pub(crate) const FSP: u16 = 0xF00A;
    pub(crate) const FOPT: u16 = 0xF00B;
    pub(crate) const CLIENT_TEXTBOX: u16 = 0xF00D;
    pub(crate) const CHILD_ANCHOR: u16 = 0xF00F;
    pub(crate) const CLIENT_ANCHOR: u16 = 0xF010;
    pub(crate) const CLIENT_DATA: u16 = 0xF011;
    pub(crate) const SECONDARY_FOPT: u16 = 0xF121;
    pub(crate) const TERTIARY_FOPT: u16 = 0xF122;
}

/// One record: an atom with data, or a container whose data is more records.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Record<'a> {
    pub(crate) version: u8,
    pub(crate) instance: u16,
    pub(crate) kind: u16,
    pub(crate) data: &'a [u8],
}

impl<'a> Record<'a> {
    pub(crate) fn is_container(&self) -> bool {
        self.version == 0xF
    }

    /// The records inside a container.
    pub(crate) fn children(&self) -> Records<'a> {
        Records::new(self.data)
    }

    /// The first child of the given type.
    pub(crate) fn child(&self, kind: u16) -> Option<Record<'a>> {
        self.children().find(|r| r.kind == kind)
    }
}

/// Iterator over consecutive records. A record whose length runs past the
/// end is cut to what is there; iteration stops at a header that does not
/// fit.
#[derive(Debug, Clone)]
pub(crate) struct Records<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Records<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// The bytes not iterated yet.
    pub(crate) fn rest(&self) -> &'a [u8] {
        self.data.get(self.pos..).unwrap_or_default()
    }
}

impl<'a> Iterator for Records<'a> {
    type Item = Record<'a>;

    fn next(&mut self) -> Option<Record<'a>> {
        let mut r = Cursor::at(self.data, self.pos);
        let ver_inst = r.u16()?;
        let kind = r.u16()?;
        let len = r.u32()? as usize;
        let start = r.pos();
        let end = start.saturating_add(len).min(self.data.len());
        self.pos = end;
        Some(Record {
            version: (ver_inst & 0xF) as u8,
            instance: ver_inst >> 4,
            kind,
            data: &self.data[start..end],
        })
    }
}

// ----- pictures --------------------------------------------------------------

/// A decoded picture.
#[derive(Debug, Clone)]
pub(crate) struct Blip {
    pub(crate) data: Vec<u8>,
    pub(crate) content_type: &'static str,
    /// Name recorded in the picture store, if any.
    pub(crate) name: Option<String>,
}

/// Decode a BLIP record. Metafiles are inflated; device-independent bitmaps
/// get the file header that makes them `.bmp` files.
pub(crate) fn blip(record: &Record<'_>, max_size: u64) -> Option<Blip> {
    // One 16-byte UID, two when the instance is odd.
    let uids = if record.instance & 1 == 1 { 32 } else { 16 };
    let (content_type, metafile) = match record.kind {
        0xF01A => ("image/emf", true),
        0xF01B => ("image/wmf", true),
        0xF01C => ("image/pict", true),
        0xF01D | 0xF02A => ("image/jpeg", false),
        0xF01E => ("image/png", false),
        0xF01F => ("image/bmp", false),
        0xF029 => ("image/tiff", false),
        _ => return None,
    };
    let mut r = Cursor::new(record.data);
    r.skip(uids)?;
    let data = if metafile {
        let size = u64::from(r.u32()?);
        r.skip(16 + 8)?;
        let saved = r.u32()? as usize;
        let compression = r.u8()?;
        r.skip(1)?;
        let raw = r.take(saved.min(r.remaining()))?;
        let mut out = if compression == 0 {
            inflate(raw, size.min(max_size))?
        } else {
            raw.to_vec()
        };
        if record.kind == 0xF01C {
            // PICT files start with a 512-byte header the BLIP leaves out.
            let mut pict = vec![0; 512];
            pict.append(&mut out);
            out = pict;
        }
        out
    } else {
        r.skip(1)?;
        let rest = r.take(r.remaining())?;
        if record.kind == 0xF01F {
            bmp_file(rest)?
        } else {
            rest.to_vec()
        }
    };
    Some(Blip {
        data,
        content_type,
        name: None,
    })
}

/// Inflate zlib data, keeping at most `limit` bytes.
pub(crate) fn inflate(raw: &[u8], limit: u64) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(raw)
        .take(limit)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

/// Prefix a packed DIB with a BITMAPFILEHEADER.
pub(crate) fn bmp_file(dib: &[u8]) -> Option<Vec<u8>> {
    let mut r = Cursor::new(dib);
    let header = r.u32()?;
    let bits_offset = if header == 12 {
        // BITMAPCOREHEADER: 3-byte palette entries.
        r.skip(4 + 2)?;
        let bit_count = r.u16()?;
        let colors = if bit_count <= 8 { 1u32 << bit_count } else { 0 };
        colors.checked_mul(3)?.checked_add(14 + header)?
    } else {
        r.skip(4 + 4 + 2)?;
        let bit_count = r.u16()?;
        let compression = r.u32()?;
        r.skip(4 + 4 + 4)?;
        let used = r.u32()?;
        let colors = match used {
            0 if bit_count <= 8 => 1u32 << bit_count,
            n => n,
        };
        // BI_BITFIELDS keeps three masks after a 40-byte header.
        let masks = if compression == 3 && header == 40 {
            12
        } else {
            0
        };
        colors
            .checked_mul(4)?
            .checked_add(header.checked_add(14 + masks)?)?
    };
    let size = u32::try_from(dib.len() + 14).ok()?;
    let mut out = Vec::with_capacity(dib.len() + 14);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&bits_offset.to_le_bytes());
    out.extend_from_slice(dib);
    Some(out)
}

/// The picture store of a drawing group: pictures by 1-based index.
#[derive(Debug, Default)]
pub(crate) struct BlipStore {
    entries: Vec<StoreEntry>,
}

#[derive(Debug)]
struct StoreEntry {
    /// The picture, when stored inside the store record.
    inline: Option<Blip>,
    /// Offset of the picture in the delay stream otherwise.
    delay: Option<usize>,
    name: Option<String>,
}

impl BlipStore {
    /// Read the store inside a drawing group container (`OfficeArtDggContainer`).
    pub(crate) fn from_drawing_group(dgg: &Record<'_>, max_size: u64) -> BlipStore {
        let Some(store) = dgg.child(rt::BSTORE_CONTAINER) else {
            return BlipStore::default();
        };
        let entries = store
            .children()
            .map(|fbse| store_entry(&fbse, max_size))
            .collect();
        BlipStore { entries }
    }

    /// The picture at a 1-based index; `delay` is the stream that pictures
    /// stored outside the drawing group live in (Word's main stream,
    /// PowerPoint's `Pictures` stream).
    pub(crate) fn get(&self, index: u32, delay: Option<&[u8]>, max_size: u64) -> Option<Blip> {
        let entry = self.entries.get((index as usize).checked_sub(1)?)?;
        let mut blip = match (&entry.inline, entry.delay, delay) {
            (Some(blip), _, _) => blip.clone(),
            (None, Some(offset), Some(stream)) => {
                let record = Records::new(stream.get(offset..)?).next()?;
                blip(&record, max_size)?
            }
            _ => return None,
        };
        blip.name = entry.name.clone();
        Some(blip)
    }
}

/// The picture of an `OfficeArtFBSE` record that carries its BLIP, or of a
/// bare BLIP record (Word keeps inline pictures this way).
pub(crate) fn embedded_blip(record: &Record<'_>, max_size: u64) -> Option<Blip> {
    if record.kind != rt::FBSE {
        return blip(record, max_size);
    }
    let entry = store_entry(record, max_size);
    let mut blip = entry.inline?;
    blip.name = entry.name;
    Some(blip)
}

fn store_entry(fbse: &Record<'_>, max_size: u64) -> StoreEntry {
    let empty = StoreEntry {
        inline: None,
        delay: None,
        name: None,
    };
    if fbse.kind != rt::FBSE {
        return empty;
    }
    let mut r = Cursor::new(fbse.data);
    let parsed = (|| {
        r.skip(1 + 1 + 16 + 2)?;
        let size = r.u32()?;
        let refs = r.u32()?;
        let delay = r.u32()? as usize;
        r.skip(1)?;
        let name_len = r.u8()? as usize;
        r.skip(2)?;
        let name = r.take(name_len)?;
        let name = Some(utf16le(name)).filter(|n| !n.trim().is_empty());
        let inline = Records::new(r.take(r.remaining())?)
            .next()
            .and_then(|record| blip(&record, max_size));
        let stored = size > 0 && refs > 0;
        Some(StoreEntry {
            delay: (inline.is_none() && stored).then_some(delay),
            inline,
            name,
        })
    })();
    parsed.unwrap_or(empty)
}

// ----- shapes ----------------------------------------------------------------

/// The properties of a shape that matter for reading it.
#[derive(Debug, Clone, Default)]
pub(crate) struct ShapeProps {
    /// 1-based index of the picture in the picture store.
    pub(crate) blip: Option<u32>,
    pub(crate) name: Option<String>,
    /// Alternative text.
    pub(crate) description: Option<String>,
    /// Hyperlink object (MS-OSHARED) attached to the shape.
    pub(crate) hyperlink: Option<Vec<u8>>,
    /// WordArt text.
    pub(crate) word_art: Option<String>,
    /// The group is a PowerPoint table (`tableProperties`).
    pub(crate) table: bool,
}

/// One shape of a drawing.
#[derive(Debug, Clone, Default)]
pub(crate) struct Shape<'a> {
    pub(crate) id: u32,
    /// The shape type (`msosptPictureFrame` = 75, `msosptTextBox` = 202, ...).
    pub(crate) shape_type: u16,
    pub(crate) flags: u32,
    pub(crate) props: ShapeProps,
    pub(crate) anchor: Option<&'a [u8]>,
    pub(crate) client_anchor: Option<&'a [u8]>,
    pub(crate) client_data: Option<&'a [u8]>,
    pub(crate) client_textbox: Option<&'a [u8]>,
    /// Members of a group shape.
    pub(crate) children: Vec<Shape<'a>>,
}

impl Shape<'_> {
    pub(crate) fn is_deleted(&self) -> bool {
        self.flags & 0x8 != 0
    }

    /// The patriarch (the drawing's root group) and the background shape
    /// are bookkeeping, not content.
    pub(crate) fn is_structural(&self) -> bool {
        self.flags & (0x4 | 0x400) != 0
    }
}

/// The shapes of a drawing container (`OfficeArtDgContainer`) or of a
/// group container (`OfficeArtSpgrContainer`), in drawing order.
pub(crate) fn shapes<'a>(container: &Record<'a>) -> Vec<Shape<'a>> {
    let mut out = Vec::new();
    collect_shapes(container, &mut out, 0);
    out
}

fn collect_shapes<'a>(container: &Record<'a>, out: &mut Vec<Shape<'a>>, depth: usize) {
    if depth > 32 {
        return;
    }
    for record in container.children() {
        match record.kind {
            rt::SPGR_CONTAINER => {
                let mut members = record.children();
                // The first shape of a group is the group itself.
                let Some(first) = members.next() else {
                    continue;
                };
                let mut group = shape(&first);
                let rest = Record {
                    data: members.rest(),
                    ..record
                };
                collect_shapes(&rest, &mut group.children, depth + 1);
                if group.is_structural() {
                    // The patriarch's members are the drawing's top level.
                    out.append(&mut group.children);
                } else {
                    out.push(group);
                }
            }
            rt::SP_CONTAINER => out.push(shape(&record)),
            _ => {}
        }
    }
}

/// Read one shape container (`OfficeArtSpContainer`).
pub(crate) fn shape<'a>(sp: &Record<'a>) -> Shape<'a> {
    let mut shape = Shape::default();
    if sp.kind != rt::SP_CONTAINER {
        return shape;
    }
    for record in sp.children() {
        match record.kind {
            rt::FSP => {
                shape.shape_type = record.instance;
                let mut r = Cursor::new(record.data);
                shape.id = r.u32().unwrap_or_default();
                shape.flags = r.u32().unwrap_or_default();
            }
            rt::FOPT | rt::SECONDARY_FOPT | rt::TERTIARY_FOPT => {
                read_props(&record, &mut shape.props)
            }
            rt::CHILD_ANCHOR => shape.anchor = Some(record.data),
            rt::CLIENT_ANCHOR => shape.client_anchor = Some(record.data),
            rt::CLIENT_DATA => shape.client_data = Some(record.data),
            rt::CLIENT_TEXTBOX => shape.client_textbox = Some(record.data),
            _ => {}
        }
    }
    shape
}

/// Read a property table: fixed 6-byte entries, then the data of complex
/// properties in the same order.
fn read_props(record: &Record<'_>, props: &mut ShapeProps) {
    let count = record.instance as usize;
    let mut r = Cursor::new(record.data);
    let mut entries = Vec::with_capacity(count.min(512));
    for _ in 0..count {
        let (Some(id), Some(value)) = (r.u16(), r.u32()) else {
            return;
        };
        entries.push((id, value));
    }
    for (id, value) in entries {
        let complex = id & 0x8000 != 0;
        let data = if complex {
            match r.take(value as usize) {
                Some(d) => Some(d),
                None => return,
            }
        } else {
            None
        };
        match (id & 0x3FFF, data) {
            (0x0104, None) => props.blip = Some(value),
            (0x0380, Some(d)) => props.name = text(d),
            (0x0381, Some(d)) => props.description = text(d),
            (0x0382, Some(d)) => props.hyperlink = Some(d.to_vec()),
            (0x00C0, Some(d)) => props.word_art = text(d),
            (0x039F, None) => props.table = value & 1 == 1,
            _ => {}
        }
    }
}

fn text(data: &[u8]) -> Option<String> {
    Some(utf16le(data)).filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(ver: u8, instance: u16, kind: u16, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(u16::from(ver) | instance << 4).to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn records_nest_and_truncated_records_are_cut() {
        let inner = record(2, 0, 0xF00A, &[1, 0, 0, 0, 0, 0, 0, 0]);
        let outer = record(0xF, 0, rt::SP_CONTAINER, &inner);
        let mut data = outer.clone();
        data.extend_from_slice(&[0, 0, 0x0B, 0xF0, 100, 0, 0, 0, 1, 2]);
        let all: Vec<_> = Records::new(&data).collect();
        assert_eq!(all.len(), 2);
        assert!(all[0].is_container());
        assert_eq!(all[0].child(rt::FSP).map(|r| r.data.len()), Some(8));
        assert_eq!(all[1].data, &[1, 2]);
    }

    #[test]
    fn png_blips_are_unwrapped() {
        let mut body = vec![0; 16];
        body.push(0xFF);
        body.extend_from_slice(b"\x89PNG\r\n\x1a\npayload");
        let data = record(0, 0x6E0, 0xF01E, &body);
        let rec = Records::new(&data).next().unwrap();
        let blip = blip(&rec, 1 << 20).unwrap();
        assert_eq!(blip.content_type, "image/png");
        assert!(blip.data.starts_with(b"\x89PNG"));
    }

    #[test]
    fn dibs_become_bmp_files() {
        let mut dib = vec![40, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 24, 0];
        dib.extend_from_slice(&[0; 24]);
        dib.extend_from_slice(&[0, 0, 255, 0]);
        let bmp = bmp_file(&dib).unwrap();
        assert!(bmp.starts_with(b"BM"));
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 54);
    }

    #[test]
    fn shape_properties_are_read() {
        let mut fopt = Vec::new();
        fopt.extend_from_slice(&(0x0104u16 | 0x4000).to_le_bytes());
        fopt.extend_from_slice(&3u32.to_le_bytes());
        let name: Vec<u8> = "Logo\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        fopt.extend_from_slice(&(0x0380u16 | 0x8000).to_le_bytes());
        fopt.extend_from_slice(&(name.len() as u32).to_le_bytes());
        fopt.extend_from_slice(&name);
        let mut sp = record(2, 75, rt::FSP, &[7, 4, 0, 0, 0, 0x0A, 0, 0]);
        sp.extend(record(3, 2, rt::FOPT, &fopt));
        let container = record(0xF, 0, rt::SP_CONTAINER, &sp);
        let dg = record(0xF, 0, rt::DG_CONTAINER, &container);
        let rec = Records::new(&dg).next().unwrap();
        let shapes = shapes(&rec);
        assert_eq!(shapes.len(), 1);
        assert_eq!(shapes[0].id, 0x407);
        assert_eq!(shapes[0].shape_type, 75);
        assert_eq!(shapes[0].props.blip, Some(3));
        assert_eq!(shapes[0].props.name.as_deref(), Some("Logo"));
    }
}
