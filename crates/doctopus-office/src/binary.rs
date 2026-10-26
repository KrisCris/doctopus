//! Bounds-checked little-endian reading for the binary Office formats.
//!
//! Every read returns `None` past the end of the data, so a parser of a
//! damaged file stops cleanly with `?` instead of panicking.

use crate::ole::text::{ansi, utf16le};

/// A reader over a byte slice.
#[derive(Debug, Clone)]
pub(crate) struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// A reader positioned at `pos` (which may be past the end).
    pub(crate) fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// Bytes left to read.
    pub(crate) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub(crate) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    pub(crate) fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    pub(crate) fn u16(&mut self) -> Option<u16> {
        self.array().map(u16::from_le_bytes)
    }

    pub(crate) fn i16(&mut self) -> Option<i16> {
        self.array().map(i16::from_le_bytes)
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        self.array().map(u32::from_le_bytes)
    }

    pub(crate) fn i32(&mut self) -> Option<i32> {
        self.array().map(i32::from_le_bytes)
    }

    pub(crate) fn u64(&mut self) -> Option<u64> {
        self.array().map(u64::from_le_bytes)
    }

    pub(crate) fn f64(&mut self) -> Option<f64> {
        self.array().map(f64::from_le_bytes)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    /// A NUL-terminated ANSI string.
    pub(crate) fn zstring(&mut self) -> Option<String> {
        let rest = self.data.get(self.pos..)?;
        let len = rest.iter().position(|&b| b == 0)?;
        let s = ansi(&rest[..len]);
        self.pos += len + 1;
        Some(s)
    }

    /// A string with a 32-bit byte length that includes the NUL.
    pub(crate) fn ansi_string(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        if len > 0x1_0000 {
            return None;
        }
        let raw = self.take(len)?;
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        Some(ansi(&raw[..end]))
    }

    /// A UTF-16 string with a 32-bit length in characters, NUL included.
    pub(crate) fn unicode_string(&mut self) -> Option<String> {
        let chars = self.u32()? as usize;
        if chars > 0x1_0000 {
            return None;
        }
        let raw = self.take(chars * 2)?;
        Some(utf16le(raw))
    }
}

/// The 16-bit value at `pos`.
pub(crate) fn u16_at(data: &[u8], pos: usize) -> Option<u16> {
    Cursor::at(data, pos).u16()
}

/// The signed 16-bit value at `pos`.
pub(crate) fn i16_at(data: &[u8], pos: usize) -> Option<i16> {
    Cursor::at(data, pos).i16()
}

/// The signed 32-bit value at `pos`.
pub(crate) fn i32_at(data: &[u8], pos: usize) -> Option<i32> {
    Cursor::at(data, pos).i32()
}

/// The 32-bit value at `pos`.
pub(crate) fn u32_at(data: &[u8], pos: usize) -> Option<u32> {
    Cursor::at(data, pos).u32()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_stop_at_the_end() {
        let mut r = Cursor::new(&[1, 0, 2, 0, 0, 0, 9]);
        assert_eq!(r.u16(), Some(1));
        assert_eq!(r.u32(), Some(2));
        assert_eq!(r.u16(), None);
        assert_eq!(r.u8(), Some(9));
        assert_eq!(r.u8(), None);
        assert_eq!(u32_at(&[0, 1, 0, 0, 0], 1), Some(1));
        assert_eq!(u16_at(&[0], 5), None);
    }
}
