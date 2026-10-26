//! Where the text and its formatting are: the piece table that assembles
//! the text from runs of the `WordDocument` stream, the PLCs that attach
//! data to character positions, and the formatted disk pages (FKPs) that
//! hold paragraph and character properties.

use std::ops::Range;

use crate::binary::{Cursor, u16_at, u32_at};

/// Size of a formatted disk page.
const PAGE: usize = 512;

/// A PLC: `n + 1` ascending character positions (or stream offsets) and
/// `n` data elements of a fixed size.
#[derive(Debug, Clone, Default)]
pub(super) struct Plc<'t> {
    cps: Vec<u32>,
    data: &'t [u8],
    size: usize,
}

impl<'t> Plc<'t> {
    pub(super) fn new(bytes: &'t [u8], size: usize) -> Self {
        if bytes.len() < 4 {
            return Self::default();
        }
        let n = (bytes.len() - 4) / (4 + size);
        let cps = (0..=n).map(|i| u32_at(bytes, i * 4).unwrap_or(0)).collect();
        let start = (n + 1) * 4;
        Self {
            cps,
            data: &bytes[start..start + n * size],
            size,
        }
    }

    /// Number of elements.
    pub(super) fn len(&self) -> usize {
        self.cps.len().saturating_sub(1)
    }

    /// The position at which element `i` starts.
    pub(super) fn cp(&self, i: usize) -> Option<u32> {
        self.cps.get(i).copied()
    }

    /// The positions element `i` covers.
    pub(super) fn range(&self, i: usize) -> Option<Range<u32>> {
        Some(self.cp(i)?..self.cp(i + 1)?)
    }

    pub(super) fn data(&self, i: usize) -> Option<&'t [u8]> {
        if i >= self.len() {
            return None;
        }
        self.data.get(i * self.size..(i + 1) * self.size)
    }

    /// The element that starts exactly at `cp`.
    pub(super) fn find(&self, cp: u32) -> Option<usize> {
        let i = self.cps[..self.len()].binary_search(&cp).ok()?;
        Some(i)
    }
}

/// One piece of the text: a run of characters stored consecutively.
#[derive(Debug, Clone, Copy)]
struct Piece {
    cp: u32,
    end: u32,
    /// Stream offset of the first character.
    fc: u32,
    /// One byte per character (Windows-1252) instead of two (UTF-16).
    compressed: bool,
    /// Property modifier applied to the whole piece.
    prm: u16,
}

/// The document text, one UTF-16 unit per character position, and how
/// positions map to stream offsets.
#[derive(Debug, Default)]
pub(super) struct Text<'d> {
    units: Vec<u16>,
    pieces: Vec<Piece>,
    /// Property groups referred to by complex property modifiers.
    grpprls: Vec<&'d [u8]>,
}

impl<'d> Text<'d> {
    /// Assemble the text from the piece table (`Clx`) in the table stream.
    pub(super) fn read(clx: &'d [u8], stream: &[u8]) -> Option<Text<'d>> {
        let mut r = Cursor::new(clx);
        let mut grpprls = Vec::new();
        // Prc entries come first, then the Pcdt.
        loop {
            match r.u8()? {
                0x01 => {
                    let len = r.u16()? as usize;
                    grpprls.push(r.take(len)?);
                }
                0x02 => break,
                _ => return None,
            }
        }
        let len = r.u32()? as usize;
        let plc = Plc::new(r.take(len.min(r.remaining()))?, 8);
        let mut pieces: Vec<Piece> = Vec::with_capacity(plc.len());
        let mut units = Vec::new();
        for i in 0..plc.len() {
            let (Some(range), Some(pcd)) = (plc.range(i), plc.data(i)) else {
                break;
            };
            // Positions must ascend; a piece cannot hold more characters
            // than the stream has bytes.
            let start = units.len() as u32;
            if range.start != start || range.end < range.start {
                break;
            }
            let count = (range.end - range.start) as usize;
            if units.len() + count > stream.len() {
                break;
            }
            let raw = u32_at(pcd, 2)?;
            let compressed = raw & 0x4000_0000 != 0;
            let fc = if compressed {
                (raw & 0x3FFF_FFFF) / 2
            } else {
                raw & 0x3FFF_FFFF
            };
            let piece = Piece {
                cp: range.start,
                end: range.end,
                fc,
                compressed,
                prm: u16_at(pcd, 6).unwrap_or(0),
            };
            decode(stream, &piece, &mut units, count);
            pieces.push(piece);
        }
        (!pieces.is_empty()).then_some(Text {
            units,
            pieces,
            grpprls,
        })
    }

    /// Number of character positions.
    pub(super) fn len(&self) -> u32 {
        self.units.len() as u32
    }

    /// The UTF-16 unit at a character position.
    pub(super) fn unit(&self, cp: u32) -> u16 {
        self.units.get(cp as usize).copied().unwrap_or(0)
    }

    fn piece(&self, cp: u32) -> Option<usize> {
        let i = self.pieces.partition_point(|p| p.end <= cp);
        (i < self.pieces.len() && self.pieces[i].cp <= cp).then_some(i)
    }

    /// The stream offset of a character and its size in bytes.
    pub(super) fn fc(&self, cp: u32) -> Option<(u32, u32)> {
        let piece = &self.pieces[self.piece(cp)?];
        let width = if piece.compressed { 1 } else { 2 };
        Some((piece.fc.checked_add((cp - piece.cp) * width)?, width))
    }

    /// The properties a piece's modifier applies to the character at `cp`.
    /// Only complex modifiers (references to property groups) are read; the
    /// single-property form is left out.
    pub(super) fn piece_sprms(&self, cp: u32) -> &'d [u8] {
        let Some(piece) = self.piece(cp).map(|i| self.pieces[i]) else {
            return &[];
        };
        if piece.prm & 1 == 0 {
            return &[];
        }
        let index = usize::from(piece.prm >> 1);
        self.grpprls.get(index).copied().unwrap_or_default()
    }

    /// Split `range` into runs that share character properties: the same
    /// piece and the same character run of the formatted disk pages.
    pub(super) fn char_runs<'b>(
        &self,
        range: Range<u32>,
        bins: &'b Bins<'d>,
    ) -> Vec<(Range<u32>, Option<&'b Bin<'d>>)> {
        let mut out: Vec<(Range<u32>, Option<&'b Bin<'d>>)> = Vec::new();
        let mut cp = range.start;
        while cp < range.end {
            let Some(index) = self.piece(cp) else {
                out.push((cp..range.end, None));
                break;
            };
            let piece = self.pieces[index];
            let width = if piece.compressed { 1 } else { 2 };
            let piece_end = piece.end.min(range.end);
            let fc = piece.fc.saturating_add((cp - piece.cp) * width);
            let (bin, fc_end) = match bins.at(fc) {
                Some(bin) => (Some(bin), bin.end),
                None => (None, bins.next_start(fc).unwrap_or(u32::MAX)),
            };
            let chars = (fc_end.saturating_sub(fc)).div_ceil(width).max(1);
            let end = cp.saturating_add(chars).min(piece_end);
            out.push((cp..end, bin));
            cp = end;
        }
        out
    }
}

/// Decode the characters of a piece.
fn decode(stream: &[u8], piece: &Piece, units: &mut Vec<u16>, count: usize) {
    let start = piece.fc as usize;
    if piece.compressed {
        let bytes = stream
            .get(start..start.saturating_add(count))
            .unwrap_or(&[]);
        let text = encoding_rs::WINDOWS_1252
            .decode_without_bom_handling(bytes)
            .0;
        units.extend(text.encode_utf16());
    } else {
        let bytes = stream
            .get(start..start.saturating_add(count * 2))
            .unwrap_or(&[]);
        units.extend(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c)),
        );
    }
    // Pad what was missing so positions stay aligned.
    let expected = piece.cp as usize + count;
    units.resize(expected, 0);
}

/// One run of a formatted disk page: properties for a range of stream
/// offsets.
#[derive(Debug, Clone, Copy)]
pub(super) struct Bin<'d> {
    fc: u32,
    end: u32,
    /// The paragraph style (paragraph runs only).
    pub(super) istd: u16,
    pub(super) grpprl: &'d [u8],
}

/// The runs of all formatted disk pages of one kind, in stream order.
#[derive(Debug, Default)]
pub(super) struct Bins<'d> {
    runs: Vec<Bin<'d>>,
}

impl<'d> Bins<'d> {
    /// Character runs from the `PlcBteChpx` bin table.
    pub(super) fn characters(table: &[u8], stream: &'d [u8]) -> Self {
        Self::read(table, stream, |page, crun, i| {
            let offset = usize::from(*page.get(4 * (crun + 1) + i)?) * 2;
            if offset == 0 {
                return Some((0, &[][..]));
            }
            let cb = usize::from(*page.get(offset)?);
            Some((0, page.get(offset + 1..offset + 1 + cb)?))
        })
    }

    /// Paragraph runs from the `PlcBtePapx` bin table.
    pub(super) fn paragraphs(table: &[u8], stream: &'d [u8]) -> Self {
        Self::read(table, stream, |page, crun, i| {
            let offset = usize::from(*page.get(4 * (crun + 1) + 13 * i)?) * 2;
            let cb = usize::from(*page.get(offset)?);
            let data = if cb == 0 {
                let cb = usize::from(*page.get(offset + 1)?);
                page.get(offset + 2..offset + 2 + 2 * cb)?
            } else {
                page.get(offset + 1..offset + 2 * cb)?
            };
            Some((u16_at(data, 0)?, data.get(2..)?))
        })
    }

    fn read(
        table: &[u8],
        stream: &'d [u8],
        entry: impl Fn(&'d [u8], usize, usize) -> Option<(u16, &'d [u8])>,
    ) -> Self {
        let bte = Plc::new(table, 4);
        let mut runs = Vec::new();
        for i in 0..bte.len() {
            let Some(pn) = bte.data(i).and_then(|d| u32_at(d, 0)) else {
                continue;
            };
            let start = (pn & 0x3F_FFFF) as usize * PAGE;
            let Some(page) = stream.get(start..start + PAGE) else {
                continue;
            };
            let crun = usize::from(page[PAGE - 1]);
            for j in 0..crun {
                let (Some(fc), Some(end)) = (u32_at(page, j * 4), u32_at(page, (j + 1) * 4)) else {
                    break;
                };
                if end <= fc {
                    continue;
                }
                let (istd, grpprl) = entry(page, crun, j).unwrap_or((0, &[]));
                runs.push(Bin {
                    fc,
                    end,
                    istd,
                    grpprl,
                });
            }
        }
        runs.sort_by_key(|r| r.fc);
        Self { runs }
    }

    /// The run holding a stream offset.
    pub(super) fn at(&self, fc: u32) -> Option<&Bin<'d>> {
        let i = self.runs.partition_point(|r| r.end <= fc);
        self.runs.get(i).filter(|r| r.fc <= fc)
    }

    /// Where the next run after `fc` starts.
    fn next_start(&self, fc: u32) -> Option<u32> {
        let i = self.runs.partition_point(|r| r.fc <= fc);
        self.runs.get(i).map(|r| r.fc)
    }

    /// Whether a paragraph run ends right after the character at `fc`.
    pub(super) fn ends_at(&self, fc: u32, width: u32) -> bool {
        self.at(fc).is_none_or(|r| r.end == fc + width)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le32(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn plcs_split_positions_and_data() {
        let mut bytes = le32(&[0, 5, 9]);
        bytes.extend([1, 2, 3, 4]);
        let plc = Plc::new(&bytes, 2);
        assert_eq!(plc.len(), 2);
        assert_eq!(plc.range(1), Some(5..9));
        assert_eq!(plc.data(1), Some(&[3, 4][..]));
        assert_eq!(plc.find(5), Some(1));
        assert_eq!(plc.find(9), None);
        assert_eq!(Plc::new(&[1, 2], 0).len(), 0);
    }

    #[test]
    fn pieces_mix_compressed_and_unicode_text() {
        // "Hi" as cp1252 at offset 8, then "€" as UTF-16 at offset 16.
        let mut stream = vec![0u8; 20];
        stream[8..10].copy_from_slice(b"Hi");
        stream[16..18].copy_from_slice(&0x20ACu16.to_le_bytes());
        let mut clx = vec![0x01, 2, 0, 0x35, 0x08, 0x02];
        let mut plc = le32(&[0, 2, 3]);
        plc.extend([0, 0]);
        plc.extend(&(0x4000_0000u32 | 16).to_le_bytes());
        plc.extend([0, 0]);
        plc.extend([0, 0]);
        plc.extend(&16u32.to_le_bytes());
        plc.extend(&1u16.to_le_bytes());
        clx.extend((plc.len() as u32).to_le_bytes());
        clx.extend(plc);

        let text = Text::read(&clx, &stream).unwrap();
        assert_eq!(text.len(), 3);
        assert_eq!(String::from_utf16_lossy(&text.units), "Hi€");
        assert_eq!(text.fc(1), Some((9, 1)));
        assert_eq!(text.fc(2), Some((16, 2)));
        assert_eq!(text.piece_sprms(0), &[] as &[u8]);
        assert_eq!(text.piece_sprms(2), &[0x35, 0x08]);
    }
}
