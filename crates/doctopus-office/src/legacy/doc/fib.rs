//! The File Information Block at the start of the `WordDocument` stream:
//! the format version, flags, the lengths of the text stories, and where
//! everything else is in the table stream.

use crate::binary::{Cursor, u16_at, u32_at};

/// Indexes of the `FibRgFcLcb` entries used here (MS-DOC 2.5.6 and later).
pub(super) mod lcb {
    pub(crate) const STSHF: usize = 1;
    pub(crate) const PLCFFND_REF: usize = 2;
    pub(crate) const PLCFFND_TXT: usize = 3;
    pub(crate) const PLCFAND_REF: usize = 4;
    pub(crate) const PLCFAND_TXT: usize = 5;
    pub(crate) const PLCFHDD: usize = 11;
    pub(crate) const PLCFBTE_CHPX: usize = 12;
    pub(crate) const PLCFBTE_PAPX: usize = 13;
    pub(crate) const STTBF_FFN: usize = 15;
    pub(crate) const CLX: usize = 33;
    pub(crate) const GRP_XST_ATN_OWNERS: usize = 36;
    pub(crate) const STTBF_ATN_BKMK: usize = 37;
    pub(crate) const PLC_SPA_MOM: usize = 40;
    pub(crate) const PLC_SPA_HDR: usize = 41;
    pub(crate) const PLCF_ATN_BKF: usize = 42;
    pub(crate) const PLCF_ATN_BKL: usize = 43;
    pub(crate) const PLCFEND_REF: usize = 46;
    pub(crate) const PLCFEND_TXT: usize = 47;
    pub(crate) const DGG_INFO: usize = 50;
    pub(crate) const PLCFTXBX_TXT: usize = 56;
    pub(crate) const PLCF_HDRTXBX_TXT: usize = 58;
    pub(crate) const PLF_LST: usize = 73;
    pub(crate) const PLF_LFO: usize = 74;
    pub(crate) const ATRD_EXTRA: usize = 112;
}

/// `wIdent` of Word 97 and later files.
const MAGIC: u16 = 0xA5EC;
/// `wIdent` of Word 6.0 and Word 95 files.
const MAGIC_WORD6: u16 = 0xA5DC;
/// The oldest `nFib` of the Word 97 format; older files are Word 6 or 95.
const WORD97: u16 = 0x00C0;

/// What the start of a `WordDocument` stream says about the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// Word 97 or later.
    Word97,
    /// Word 6.0 or Word 95, a different format.
    Word6,
    NotWord,
}

/// Identify the format version from the first bytes of the stream.
pub(super) fn kind(data: &[u8]) -> Kind {
    match (u16_at(data, 0), u16_at(data, 2)) {
        (Some(MAGIC), Some(n_fib)) if n_fib >= WORD97 => Kind::Word97,
        (Some(MAGIC | MAGIC_WORD6), Some(_)) => Kind::Word6,
        _ => Kind::NotWord,
    }
}

/// What the rest of the reader needs from the FIB.
#[derive(Debug, Clone)]
pub(super) struct Fib {
    /// Encrypted, or obfuscated with the XOR method.
    pub(super) encrypted: bool,
    /// `1Table` or `0Table`.
    pub(super) table_stream: &'static str,
    /// Lengths of the stories, in characters.
    pub(super) ccp: StoryLengths,
    fc_lcb: Vec<(u32, u32)>,
}

/// Character counts of the stories, which follow each other in this order.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct StoryLengths {
    pub(super) text: u32,
    pub(super) footnotes: u32,
    pub(super) headers: u32,
    pub(super) macros: u32,
    pub(super) comments: u32,
    pub(super) endnotes: u32,
    pub(super) text_boxes: u32,
    pub(super) header_text_boxes: u32,
}

impl Fib {
    /// Read the FIB; `None` when the stream does not start with one.
    pub(super) fn read(data: &[u8]) -> Option<Fib> {
        let mut r = Cursor::new(data);
        if r.u16()? != MAGIC {
            return None;
        }
        r.skip(2)?;
        let flags = u16_at(data, 10)?;
        r.seek(32);
        let csw = usize::from(r.u16()?);
        r.skip(csw * 2)?;
        let cslw = usize::from(r.u16()?);
        let rg_lw = r.take(cslw * 4)?;
        let lw = |i: usize| u32_at(rg_lw, i * 4).unwrap_or(0);
        let ccp = StoryLengths {
            text: lw(3),
            footnotes: lw(4),
            headers: lw(5),
            macros: lw(6),
            comments: lw(7),
            endnotes: lw(8),
            text_boxes: lw(9),
            header_text_boxes: lw(10),
        };
        let pairs = usize::from(r.u16()?);
        let mut fc_lcb = Vec::with_capacity(pairs);
        for _ in 0..pairs {
            let (Some(fc), Some(lcb)) = (r.u32(), r.u32()) else {
                break;
            };
            fc_lcb.push((fc, lcb));
        }
        Some(Fib {
            // fObfuscated only qualifies fEncrypted.
            encrypted: flags & 0x0100 != 0,
            table_stream: if flags & 0x0200 != 0 {
                "/1Table"
            } else {
                "/0Table"
            },
            ccp,
            fc_lcb,
        })
    }

    /// The bytes of a `FibRgFcLcb` entry within the table stream, if the
    /// entry is set and in range.
    pub(super) fn slice<'t>(&self, table: &'t [u8], index: usize) -> Option<&'t [u8]> {
        let (fc, lcb) = *self.fc_lcb.get(index)?;
        if lcb == 0 {
            return None;
        }
        table.get(fc as usize..(fc as usize).checked_add(lcb as usize)?)
    }

    /// The table stream from the start of an entry to its end, for the
    /// structures that continue past their recorded length (list levels
    /// follow the list table).
    pub(super) fn tail<'t>(&self, table: &'t [u8], index: usize) -> Option<&'t [u8]> {
        let (fc, lcb) = *self.fc_lcb.get(index)?;
        if lcb == 0 {
            return None;
        }
        table.get(fc as usize..)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_versions_flags_and_entries() {
        let mut data = vec![0; 32];
        data[0..2].copy_from_slice(&MAGIC.to_le_bytes());
        data[2..4].copy_from_slice(&0x00C1u16.to_le_bytes());
        data[10..12].copy_from_slice(&0x0200u16.to_le_bytes());
        data.extend(14u16.to_le_bytes());
        data.extend([0; 28]);
        data.extend(22u16.to_le_bytes());
        let mut lw = [0u32; 22];
        lw[3] = 120;
        lw[4] = 7;
        data.extend(lw.iter().flat_map(|v| v.to_le_bytes()));
        data.extend(2u16.to_le_bytes());
        data.extend([0u32, 0, 4, 3].iter().flat_map(|v| v.to_le_bytes()));

        assert_eq!(kind(&data), Kind::Word97);
        let fib = Fib::read(&data).unwrap();
        assert_eq!(fib.table_stream, "/1Table");
        assert!(!fib.encrypted);
        assert_eq!((fib.ccp.text, fib.ccp.footnotes), (120, 7));
        assert_eq!(fib.slice(b"abcdefgh", 1), Some(&b"efg"[..]));
        assert_eq!(fib.slice(b"abcdefgh", 0), None);
        assert_eq!(fib.tail(b"abcdefgh", 1), Some(&b"efgh"[..]));
        assert!(Fib::read(&[0; 64]).is_none());
        assert_eq!(kind(&[0xEC, 0xA5, 0x65, 0]), Kind::Word6);
        assert_eq!(kind(&[0xDC, 0xA5, 0x65, 0]), Kind::Word6);
        assert_eq!(kind(&[0; 4]), Kind::NotWord);
    }
}
