use std::fmt;
use std::str::FromStr;

use serde::{Serialize, Serializer};

/// A zero-based cell coordinate, displayed in A1 notation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellRef {
    /// Zero-based row (`A1` is row 0).
    pub row: u32,
    /// Zero-based column (`A1` is column 0).
    pub col: u32,
}

impl CellRef {
    /// A cell coordinate.
    pub const fn new(row: u32, col: u32) -> Self {
        Self { row, col }
    }

    /// Column letters for a zero-based column index (`0` → `A`, `27` → `AB`).
    pub fn column_name(col: u32) -> String {
        let mut n = col as u64 + 1;
        let mut letters = Vec::new();
        while n > 0 {
            let rem = ((n - 1) % 26) as u8;
            letters.push(b'A' + rem);
            n = (n - 1) / 26;
        }
        letters.reverse();
        String::from_utf8(letters).unwrap_or_default()
    }

    /// Parse column letters (`"AB"` → `27`). Case-insensitive.
    pub fn parse_column(letters: &str) -> Option<u32> {
        if letters.is_empty() || letters.len() > 3 {
            return None;
        }
        let mut col: u32 = 0;
        for b in letters.bytes() {
            if !b.is_ascii_alphabetic() {
                return None;
            }
            col = col * 26 + u32::from(b.to_ascii_uppercase() - b'A') + 1;
        }
        Some(col - 1)
    }
}

impl fmt::Display for CellRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", Self::column_name(self.col), self.row + 1)
    }
}

impl FromStr for CellRef {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim().replace('$', "");
        let split = s
            .find(|c: char| c.is_ascii_digit())
            .ok_or_else(|| format!("`{s}` is not a cell reference like B7"))?;
        let (letters, digits) = s.split_at(split);
        let col = Self::parse_column(letters)
            .ok_or_else(|| format!("`{s}` is not a cell reference like B7"))?;
        let row: u32 = digits
            .parse()
            .ok()
            .filter(|r| *r >= 1)
            .ok_or_else(|| format!("`{s}` is not a cell reference like B7"))?;
        Ok(Self::new(row - 1, col))
    }
}

impl Serialize for CellRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// An inclusive rectangle of cells, displayed as `A1:C10`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellRange {
    /// Top-left cell.
    pub start: CellRef,
    /// Bottom-right cell.
    pub end: CellRef,
}

impl CellRange {
    /// A normalised range (start is top-left whatever the argument order).
    pub fn new(a: CellRef, b: CellRef) -> Self {
        Self {
            start: CellRef::new(a.row.min(b.row), a.col.min(b.col)),
            end: CellRef::new(a.row.max(b.row), a.col.max(b.col)),
        }
    }

    /// Whether the cell lies inside the range.
    pub fn contains(&self, row: u32, col: u32) -> bool {
        (self.start.row..=self.end.row).contains(&row)
            && (self.start.col..=self.end.col).contains(&col)
    }

    /// Number of rows covered.
    pub fn height(&self) -> u32 {
        self.end.row - self.start.row + 1
    }

    /// Number of columns covered.
    pub fn width(&self) -> u32 {
        self.end.col - self.start.col + 1
    }
}

impl fmt::Display for CellRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start == self.end {
            write!(f, "{}", self.start)
        } else {
            write!(f, "{}:{}", self.start, self.end)
        }
    }
}

impl FromStr for CellRange {
    type Err = String;

    /// Accepts `B7`, `A1:C10`, whole columns `A:C` and whole rows `3:9`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const MAX_ROW: u32 = 1_048_575;
        const MAX_COL: u32 = 16_383;
        let s = s.trim();
        let (a, b) = s.split_once(':').unwrap_or((s, s));
        let all_letters = |x: &str| !x.is_empty() && x.bytes().all(|c| c.is_ascii_alphabetic());
        let all_digits = |x: &str| !x.is_empty() && x.bytes().all(|c| c.is_ascii_digit());
        if all_letters(a) && all_letters(b) {
            let bad = || format!("`{s}` is not a column range like A:C");
            let c1 = CellRef::parse_column(a).ok_or_else(bad)?;
            let c2 = CellRef::parse_column(b).ok_or_else(bad)?;
            return Ok(Self::new(CellRef::new(0, c1), CellRef::new(MAX_ROW, c2)));
        }
        if all_digits(a) && all_digits(b) {
            let bad = || format!("`{s}` is not a row range like 3:9");
            let r1: u32 = a.parse().ok().filter(|r| *r >= 1).ok_or_else(bad)?;
            let r2: u32 = b.parse().ok().filter(|r| *r >= 1).ok_or_else(bad)?;
            return Ok(Self::new(
                CellRef::new(r1 - 1, 0),
                CellRef::new(r2 - 1, MAX_COL),
            ));
        }
        Ok(Self::new(a.parse()?, b.parse()?))
    }
}

impl Serialize for CellRange {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_names_round_trip() {
        for (col, name) in [
            (0, "A"),
            (25, "Z"),
            (26, "AA"),
            (27, "AB"),
            (701, "ZZ"),
            (702, "AAA"),
        ] {
            assert_eq!(CellRef::column_name(col), name);
            assert_eq!(CellRef::parse_column(name), Some(col));
        }
    }

    #[test]
    fn parses_ranges() {
        let r: CellRange = "b2:$D$10".parse().unwrap();
        assert_eq!(r.to_string(), "B2:D10");
        assert!(r.contains(1, 1) && r.contains(9, 3) && !r.contains(10, 3));
        let cols: CellRange = "A:C".parse().unwrap();
        assert!(cols.contains(500_000, 2));
        let rows: CellRange = "3:9".parse().unwrap();
        assert!(rows.contains(2, 100) && !rows.contains(1, 0));
        assert!("1A".parse::<CellRange>().is_err());
    }
}
