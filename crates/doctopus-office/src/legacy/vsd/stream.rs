//! The container layer of a Visio binary file: the header, the tree of
//! pointers to (often compressed) streams inside the `VisioDocument`
//! stream, and the chunks those streams hold.
//!
//! The format is not publicly documented; the layout follows what libvisio
//! and Apache POI (HDGF) learned from real files.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use crate::binary::{Cursor, u16_at, u32_at};

/// The text every Visio binary file starts with.
const SIGNATURE: &[u8] = b"Visio (TM) Drawing\r\n";
/// Where the version number and the pointer to the trailer stream are.
const VERSION_AT: usize = 0x1A;
const TRAILER_AT: usize = 0x24;

/// The sliding window of the LZ77 compression, and where its references
/// start counting.
const WINDOW: usize = 4096;
const WINDOW_START: usize = 4078;
/// The block header in front of the data of a compressed stream.
const COMPRESSED_HEADER: usize = 4;

/// Streams read at most per file; a damaged pointer tree can otherwise
/// reach the same stream many times over.
const MAX_STREAMS: usize = 200_000;

/// The file format generation, from the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Version {
    /// Visio 5.
    V5,
    /// Visio 2000 and 2002.
    V6,
    /// Visio 2003 to 2010.
    V11,
}

/// What the header says about the file.
pub(super) enum Header {
    Visio(Version),
    /// An older or unknown version number.
    Unsupported(u16),
    NotVisio,
}

pub(super) fn header(doc: &[u8]) -> Header {
    if !doc.starts_with(SIGNATURE) {
        return Header::NotVisio;
    }
    match u16_at(doc, VERSION_AT) {
        Some(5) => Header::Visio(Version::V5),
        Some(6) => Header::Visio(Version::V6),
        Some(11) => Header::Visio(Version::V11),
        Some(v) => Header::Unsupported(v),
        None => Header::NotVisio,
    }
}

/// A reference to a stream inside the `VisioDocument` stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Pointer {
    /// What the stream holds (a chunk type).
    pub(super) kind: u32,
    pub(super) offset: u32,
    pub(super) length: u32,
    pub(super) format: u16,
}

/// How a stream's data is laid out (the high nibble of the format).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Layout {
    /// One block of data.
    Blob,
    /// One block of data that also lists pointers to further streams.
    Pointers,
    /// A sequence of chunks.
    Chunks,
    Unknown,
}

impl Pointer {
    pub(super) fn layout(&self) -> Layout {
        match (self.format >> 4) & 0xF {
            0x0 | 0x4 => Layout::Blob,
            0x5 => Layout::Pointers,
            0x8 | 0xC | 0xD => Layout::Chunks,
            _ => Layout::Unknown,
        }
    }

    fn compressed(&self) -> bool {
        self.format & 2 != 0
    }
}

/// The streams of one file, decompressed on first use.
pub(super) struct Streams<'d> {
    doc: &'d [u8],
    pub(super) version: Version,
    max_size: usize,
    cache: HashMap<Pointer, Rc<[u8]>>,
    reads: usize,
}

impl<'d> Streams<'d> {
    pub(super) fn new(doc: &'d [u8], version: Version, max_size: usize) -> Self {
        Self {
            doc,
            version,
            max_size,
            cache: HashMap::new(),
            reads: 0,
        }
    }

    /// The pointer to the trailer stream, the root of the tree.
    pub(super) fn trailer(&self) -> Option<Pointer> {
        self.pointer_at(self.doc, TRAILER_AT)
    }

    fn pointer_at(&self, data: &[u8], pos: usize) -> Option<Pointer> {
        let mut r = Cursor::at(data, pos);
        Some(match self.version {
            Version::V5 => {
                let kind = u32::from(r.u16()? & 0xFF);
                let format = r.u16()? & 0xFF;
                r.skip(4)?;
                Pointer {
                    kind,
                    format,
                    offset: r.u32()?,
                    length: r.u32()?,
                }
            }
            Version::V6 | Version::V11 => {
                let kind = r.u32()?;
                r.skip(4)?;
                Pointer {
                    kind,
                    offset: r.u32()?,
                    length: r.u32()?,
                    format: r.u16()?,
                }
            }
        })
    }

    fn pointer_size(&self) -> usize {
        match self.version {
            Version::V5 => 16,
            Version::V6 | Version::V11 => 18,
        }
    }

    /// The data of a stream, decompressed; `None` when the pointer leads
    /// outside the file or the read budget is spent.
    ///
    /// Decompressed data starts with a 4-byte block header. Streams of
    /// chunks keep it, since it is the start of their first chunk; other
    /// streams drop it.
    pub(super) fn data(&mut self, ptr: &Pointer) -> Option<Rc<[u8]>> {
        if let Some(data) = self.cache.get(ptr) {
            return Some(data.clone());
        }
        self.reads += 1;
        if self.reads > MAX_STREAMS {
            return None;
        }
        let start = ptr.offset as usize;
        let end = start.checked_add(ptr.length as usize)?.min(self.doc.len());
        let raw = self.doc.get(start..end)?;
        let data: Rc<[u8]> = if ptr.compressed() {
            let mut out = decompress(raw, self.max_size);
            if ptr.layout() != Layout::Chunks {
                out.drain(..COMPRESSED_HEADER.min(out.len()));
            }
            out.into()
        } else {
            raw.into()
        };
        self.cache.insert(*ptr, data.clone());
        Some(data)
    }

    /// The pointers a stream lists, in the order the file gives (when it
    /// gives one), then the rest in index order; each with its index.
    pub(super) fn pointers(&self, data: &[u8], kind: u32) -> Vec<(u32, Pointer)> {
        let (mut at, count, order) = match self.version {
            Version::V5 => {
                let Some(count) = u16_at(data, v5_pointers_at(kind)) else {
                    return Vec::new();
                };
                (v5_pointers_at(kind) + 2, u32::from(count), 0)
            }
            Version::V6 | Version::V11 => {
                let Some(offset) = u32_at(data, 0).map(|o| o as usize) else {
                    return Vec::new();
                };
                let (Some(order), Some(count)) = (
                    offset.checked_sub(4).and_then(|at| u32_at(data, at)),
                    u32_at(data, offset),
                ) else {
                    return Vec::new();
                };
                (offset + 8, count, order)
            }
        };
        let size = self.pointer_size();
        let count = (count as usize).min(data.len().saturating_sub(at) / size);
        let mut listed = Vec::with_capacity(count);
        for i in 0..count {
            if let Some(ptr) = self.pointer_at(data, at).filter(|p| p.kind != 0) {
                listed.push((i as u32, Some(ptr)));
            }
            at += size;
        }
        let mut out = Vec::with_capacity(listed.len());
        if order > 1 {
            let order = (order as usize).min(data.len().saturating_sub(at) / 4);
            for k in 0..order {
                let Some(index) = u32_at(data, at + 4 * k) else {
                    break;
                };
                if let Some(entry) = listed.iter_mut().find(|(i, _)| *i == index)
                    && let Some(ptr) = entry.1.take()
                {
                    out.push((index, ptr));
                }
            }
        }
        out.extend(listed.into_iter().filter_map(|(i, p)| Some((i, p?))));
        out
    }

    /// The chunks of a stream (in version 5, list chunks are followed by
    /// their records, one level deeper).
    pub(super) fn chunks(&self, data: &[u8]) -> Vec<Chunk> {
        let mut out = Vec::new();
        let mut pos = 0;
        while pos < data.len() {
            // Chunks may be padded with zeros.
            match data[pos..].iter().position(|&b| b != 0) {
                Some(skip) => pos += skip,
                None => break,
            }
            let Some((chunk, next)) = (match self.version {
                Version::V5 => chunk_v5(data, pos),
                Version::V6 => chunk_v6(data, pos, false),
                Version::V11 => chunk_v6(data, pos, true),
            }) else {
                break;
            };
            out.push(chunk.clone());
            if self.version == Version::V5 && V5_LISTS.contains(&chunk.kind) {
                out.extend(records_v5(data, chunk.data.clone(), chunk.level + 1));
            }
            pos = next;
        }
        out
    }
}

/// One chunk: a typed record of a stream, as ranges of the stream's data.
#[derive(Debug, Clone)]
pub(super) struct Chunk {
    pub(super) kind: u32,
    /// [`NO_ID`] for the chunk that stands for its whole stream.
    pub(super) id: u32,
    pub(super) level: u16,
    /// The chunk's data.
    pub(super) data: Range<usize>,
    /// The data and the trailer after it, which some chunks overrun.
    pub(super) extended: Range<usize>,
}

/// The id of a chunk that has none.
pub(super) const NO_ID: u32 = u32::MAX;

/// Chunk types that always carry a trailer (child lists and the like).
const V6_TRAILERS: &[u32] = &[
    0x0D, 0x2C, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F, 0x70, 0x71,
    0x72, 0x73, 0x76,
];
const V11_TRAILERS: &[u32] = &[0x2C, 0x65, 0x66, 0x69, 0x6A, 0x6B, 0x70, 0x71];
/// Version 11 chunk types followed by a 4-byte separator.
const V11_SEPARATED: &[u32] = &[
    0x64, 0x65, 0x66, 0x69, 0x6A, 0x6B, 0x6F, 0x71, 0x92, 0xA9, 0xB4, 0xB6, 0xB9, 0xC7,
];
/// Chunk types that never have a trailer (OLE data, name indexes, names,
/// shape data).
const NO_TRAILER: &[u32] = &[0x1F, 0xC9, 0x2D, 0xD1];

/// A version 6 or 11 chunk header (19 bytes) at `pos`, and where the next
/// chunk may start.
fn chunk_v6(data: &[u8], pos: usize, v11: bool) -> Option<(Chunk, usize)> {
    let mut r = Cursor::at(data, pos);
    let kind = r.u32()?;
    let id = r.u32()?;
    let list = r.u32()?;
    let length = r.u32()? as usize;
    let level = r.u16()?;
    let unknown = r.u8()?;
    let mut trailer = 0;
    if v11 {
        if list != 0 || V11_TRAILERS.contains(&kind) {
            trailer += 8;
        }
        if list != 0
            || (level == 2 && unknown == 0x55)
            || (level == 2 && unknown == 0x54 && kind == 0xAA)
            || (level == 3 && unknown != 0x50 && unknown != 0x54)
        {
            trailer += 4;
        }
        if V11_SEPARATED.contains(&kind) && trailer != 4 && trailer != 12 {
            trailer += 4;
        }
    } else if list != 0 || V6_TRAILERS.contains(&kind) {
        trailer = 8;
    }
    if NO_TRAILER[..if v11 { 4 } else { 2 }].contains(&kind) {
        trailer = 0;
    }
    Some(chunk(data.len(), kind, id, level, r.pos(), length, trailer))
}

/// A version 5 chunk header (12 bytes) at `pos`.
fn chunk_v5(data: &[u8], pos: usize) -> Option<(Chunk, usize)> {
    let mut r = Cursor::at(data, pos);
    let kind = u32::from(r.u16()?);
    let id = match r.u16()? {
        u16::MAX => NO_ID,
        id => u32::from(id),
    };
    let level = u16::from(r.u8()?);
    r.skip(1 + 2)?;
    let length = r.u32()? as usize;
    Some(chunk(data.len(), kind, id, level, r.pos(), length, 0))
}

fn chunk(
    size: usize,
    kind: u32,
    id: u32,
    level: u16,
    start: usize,
    length: usize,
    trailer: usize,
) -> (Chunk, usize) {
    let end = start.saturating_add(length);
    let next = end.saturating_add(trailer);
    let chunk = Chunk {
        kind,
        id,
        level,
        data: start..end.min(size),
        extended: start..next.min(size),
    };
    (chunk, next)
}

/// Version 5 list chunks, which hold their members as records.
pub(super) const V5_LISTS: &[u32] = &[0x32, 0x65, 0x66, 0x68, 0x69, 0x6A, 0x6B, 0x6C];

/// The records of a version 5 list chunk (or list stream) occupying
/// `within`. A table at the end gives each record's type and start; a
/// record runs to the start of the next.
pub(super) fn records_v5(data: &[u8], within: Range<usize>, level: u16) -> Vec<Chunk> {
    let Some(list) = data.get(within.clone()) else {
        return Vec::new();
    };
    let end = list.len();
    let Some(count) = end.checked_sub(4).and_then(|at| u16_at(list, at)) else {
        return Vec::new();
    };
    let Some(table) = end
        .checked_sub(4 * (usize::from(count) + 1))
        .filter(|&t| t > 0)
    else {
        return Vec::new();
    };
    let limit = u16_at(list, end - 2).map_or(table, |l| usize::from(l).min(table));
    let mut starts = Vec::with_capacity(usize::from(count));
    for i in 0..usize::from(count) {
        let (Some(kind), Some(offset)) =
            (u16_at(list, table + 4 * i), u16_at(list, table + 4 * i + 2))
        else {
            break;
        };
        starts.push((usize::from(offset).next_multiple_of(4), u32::from(kind)));
    }
    starts.sort_unstable();
    starts.dedup_by_key(|(start, _)| *start);
    let mut out = Vec::new();
    for (i, &(start, kind)) in starts.iter().enumerate() {
        let next = starts.get(i + 1).map_or(limit, |(s, _)| (*s).min(limit));
        if start >= next {
            continue;
        }
        let range = within.start + start..within.start + next;
        out.push(Chunk {
            kind,
            id: i as u32,
            level,
            data: range.clone(),
            extended: range,
        });
    }
    out
}

/// Where a version 5 stream of type `kind` lists its pointers.
fn v5_pointers_at(kind: u32) -> usize {
    match kind {
        0x14 => 0x82,
        0x15 => 0x42,
        0x18 => 0x2E,
        0x1A => 0x12,
        0x1D | 0x4E => 0x1E,
        0x1E => 0x36,
        k if k > 0x45 => 0x1E,
        _ => 0x0A,
    }
}

/// Undo the LZ77 compression of a stream, stopping at `max_size` bytes.
///
/// Each flag byte announces eight items, low bit first: a set bit is a
/// literal byte, a clear bit a two-byte reference to 3 to 18 bytes of a
/// 4096-byte window.
pub(super) fn decompress(data: &[u8], max_size: usize) -> Vec<u8> {
    let mut window = [0u8; WINDOW];
    let mut out = Vec::with_capacity(data.len().saturating_mul(2).min(max_size));
    let mut pos = 0usize;
    let mut i = 0;
    'outer: while i < data.len() {
        let flags = data[i];
        i += 1;
        for bit in 0..8 {
            if out.len() >= max_size {
                break 'outer;
            }
            if flags & (1 << bit) != 0 {
                let Some(&byte) = data.get(i) else {
                    break 'outer;
                };
                i += 1;
                window[pos % WINDOW] = byte;
                out.push(byte);
                pos += 1;
            } else {
                let (Some(&lo), Some(&hi)) = (data.get(i), data.get(i + 1)) else {
                    break 'outer;
                };
                i += 2;
                let length = usize::from(hi & 0x0F) + 3;
                let mut from = (usize::from(hi & 0xF0) << 4) | usize::from(lo);
                from = if from > WINDOW_START {
                    from - WINDOW_START
                } else {
                    from + (WINDOW - WINDOW_START)
                };
                for k in 0..length {
                    let byte = window[(from + k) % WINDOW];
                    window[(pos + k) % WINDOW] = byte;
                    out.push(byte);
                }
                pos += length;
            }
        }
    }
    out.truncate(max_size);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_to_the_start_of_the_output() {
        // References count from 18 bytes before the first output byte, so
        // the start of the output is stored as 4078 (0xFEE); the low
        // nibble of the second byte is the length minus 3.
        let data = [0b0000_0111, b'x', b'y', b'z', 0xEE, 0xF0];
        assert_eq!(decompress(&data, 100), b"xyzxyz");
    }

    #[test]
    fn decompression_stops_at_the_limit() {
        let data = [0xFF, 1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(decompress(&data, 5), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn v11_chunk_headers_and_trailers() {
        let mut data = Vec::new();
        // A shape list (0x65): 8-byte trailer and a 4-byte separator.
        data.extend_from_slice(&0x65u32.to_le_bytes());
        data.extend_from_slice(&7u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.push(0x50);
        data.extend_from_slice(&[0xAA, 0xBB]);
        data.extend_from_slice(&[1; 12]);
        // A text chunk (0x0E) after zero padding.
        data.extend_from_slice(&[0, 0]);
        data.extend_from_slice(&0x0Eu32.to_le_bytes());
        data.extend_from_slice(&3u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&2u16.to_le_bytes());
        data.push(0x50);
        data.push(b'h');
        let streams = Streams::new(&[], Version::V11, 1 << 20);
        let chunks = streams.chunks(&data);
        assert_eq!(chunks.len(), 2);
        assert_eq!(
            (chunks[0].kind, chunks[0].id, chunks[0].level),
            (0x65, 7, 1)
        );
        assert_eq!(&data[chunks[0].data.clone()], [0xAA, 0xBB]);
        assert_eq!(chunks[0].extended.len(), 14);
        assert_eq!((chunks[1].kind, chunks[1].level), (0x0E, 2));
        assert_eq!(&data[chunks[1].data.clone()], b"h");
    }

    #[test]
    fn pointer_lists_follow_the_given_order() {
        let mut data = vec![0u8; 8];
        data[0..4].copy_from_slice(&8u32.to_le_bytes());
        // Order list size at offset - 4, count at offset, 4 bytes skipped.
        data[4..8].copy_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&[0; 4]);
        for kind in [0x15u32, 0x16] {
            data.extend_from_slice(&kind.to_le_bytes());
            data.extend_from_slice(&[0; 4]);
            data.extend_from_slice(&100u32.to_le_bytes());
            data.extend_from_slice(&10u32.to_le_bytes());
            data.extend_from_slice(&0x52u16.to_le_bytes());
        }
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        let streams = Streams::new(&[], Version::V11, 1 << 20);
        let kinds: Vec<(u32, u32)> = streams
            .pointers(&data, 0x14)
            .iter()
            .map(|(i, p)| (*i, p.kind))
            .collect();
        assert_eq!(kinds, [(1, 0x16), (0, 0x15)]);
    }
}
