//! Single property modifiers (sprms) and the paragraph, character and
//! table-row properties built from them.

use crate::binary::{Cursor, i16_at, u16_at, u32_at};

/// The sprms read here.
pub(super) mod sprm {
    pub(crate) const P_ISTD: u16 = 0x4600;
    pub(crate) const P_ILVL: u16 = 0x260A;
    pub(crate) const P_ILFO: u16 = 0x460B;
    pub(crate) const P_IN_TABLE: u16 = 0x2416;
    pub(crate) const P_TTP: u16 = 0x2417;
    pub(crate) const P_OUT_LVL: u16 = 0x2640;
    pub(crate) const P_ITAP: u16 = 0x6649;
    pub(crate) const P_DTAP: u16 = 0x664A;
    pub(crate) const P_INNER_TABLE_CELL: u16 = 0x244B;
    pub(crate) const P_INNER_TTP: u16 = 0x244C;
    pub(crate) const P_CHG_TABS: u16 = 0xC615;
    pub(crate) const T_DEF_TABLE: u16 = 0xD608;
    pub(crate) const T_DEF_TABLE10: u16 = 0xD606;
    pub(crate) const T_TABLE_HEADER: u16 = 0x3404;
    pub(crate) const T_VERT_MERGE: u16 = 0xD62B;
    pub(crate) const C_RMARK_DEL: u16 = 0x0800;
    pub(crate) const C_RMARK_INS: u16 = 0x0801;
    pub(crate) const C_PIC_LOCATION: u16 = 0x6A03;
    pub(crate) const C_DATA: u16 = 0x0806;
    pub(crate) const C_OLE2: u16 = 0x080A;
    pub(crate) const C_BOLD: u16 = 0x0835;
    pub(crate) const C_ITALIC: u16 = 0x0836;
    pub(crate) const C_STRIKE: u16 = 0x0837;
    pub(crate) const C_VANISH: u16 = 0x083C;
    pub(crate) const C_SPEC: u16 = 0x0855;
    pub(crate) const C_DSTRIKE: u16 = 0x2A53;
    pub(crate) const C_ISS: u16 = 0x2A48;
    pub(crate) const C_ISTD: u16 = 0x4A30;
    pub(crate) const C_SYMBOL: u16 = 0x6A09;
    pub(crate) const C_FONT: u16 = 0x4A4F;
}

/// One property modifier and its operand.
#[derive(Debug, Clone, Copy)]
pub(super) struct Sprm<'a> {
    pub(super) op: u16,
    pub(super) arg: &'a [u8],
}

impl Sprm<'_> {
    fn u8(&self) -> u8 {
        self.arg.first().copied().unwrap_or(0)
    }

    fn u16(&self) -> u16 {
        u16_at(self.arg, 0).unwrap_or(0)
    }

    fn u32(&self) -> u32 {
        u32_at(self.arg, 0).unwrap_or(0)
    }

    fn flag(&self) -> bool {
        self.u8() != 0
    }
}

/// Iterate over the sprms of a property group; a truncated sprm ends it.
pub(super) fn sprms(grpprl: &[u8]) -> impl Iterator<Item = Sprm<'_>> {
    let mut r = Cursor::new(grpprl);
    std::iter::from_fn(move || {
        let op = r.u16()?;
        let size = match op >> 13 {
            0 | 1 => 1,
            2 | 4 | 5 => 2,
            3 => 4,
            7 => 3,
            _ => match op {
                // The size of these is a 16-bit count that includes itself
                // minus one.
                sprm::T_DEF_TABLE | sprm::T_DEF_TABLE10 => usize::from(r.u16()?).saturating_sub(1),
                sprm::P_CHG_TABS => tabs_size(&mut r)?,
                _ => usize::from(r.u8()?),
            },
        };
        Some(Sprm {
            op,
            arg: r.take(size)?,
        })
    })
}

/// `sprmPChgTabs` records its size in one byte unless the size is 255 or
/// more; then it must be worked out from the tab counts.
fn tabs_size(r: &mut Cursor<'_>) -> Option<usize> {
    let cb = usize::from(r.u8()?);
    if cb != 255 {
        return Some(cb);
    }
    let mut probe = r.clone();
    let deleted = usize::from(probe.u8()?);
    probe.skip(deleted * 4)?;
    let added = usize::from(probe.u8()?);
    Some(1 + deleted * 4 + 1 + added * 3)
}

/// Character formatting that a style sets (`None` where it leaves the
/// property alone).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CharStyle {
    pub(super) bold: Option<bool>,
    pub(super) italic: Option<bool>,
    pub(super) strike: Option<bool>,
    pub(super) vanish: Option<bool>,
    /// Superscript (1) or subscript (2).
    pub(super) iss: Option<u8>,
    pub(super) font: Option<u16>,
}

impl CharStyle {
    /// Apply a style's own character sprms on top of its base style.
    pub(super) fn apply(&mut self, grpprl: &[u8]) {
        let base = *self;
        for s in sprms(grpprl) {
            match s.op {
                sprm::C_BOLD => self.bold = Some(toggle(s.u8(), base.bold)),
                sprm::C_ITALIC => self.italic = Some(toggle(s.u8(), base.italic)),
                sprm::C_STRIKE | sprm::C_DSTRIKE => self.strike = Some(toggle(s.u8(), base.strike)),
                sprm::C_VANISH => self.vanish = Some(toggle(s.u8(), base.vanish)),
                sprm::C_ISS => self.iss = Some(s.u8()),
                sprm::C_FONT => self.font = Some(s.u16()),
                _ => {}
            }
        }
    }
}

/// A toggle operand: off, on, or the style's value (0x80) or its opposite
/// (0x81).
fn toggle(value: u8, style: Option<bool>) -> bool {
    let style = style.unwrap_or(false);
    match value {
        0 => false,
        0x80 => style,
        0x81 => !style,
        _ => true,
    }
}

/// The properties of a run of characters.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CharProps {
    /// Bold, italic and struck through as displayed, leaving out what only
    /// the paragraph style sets (a heading style's bold says nothing the
    /// heading does not).
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) strike: bool,
    pub(super) superscript: bool,
    pub(super) subscript: bool,
    pub(super) hidden: bool,
    pub(super) deleted: bool,
    pub(super) inserted: bool,
    /// The character is special: a picture, a note reference, a drawing
    /// anchor, a symbol.
    pub(super) special: bool,
    pub(super) ole: bool,
    /// The character holds form field data, not a picture.
    pub(super) data: bool,
    /// Data stream offset of a picture, or the id of an OLE object.
    pub(super) pic: Option<u32>,
    /// Symbol character: font index and character.
    pub(super) symbol: Option<(u16, u16)>,
    pub(super) font: Option<u16>,
}

impl CharProps {
    /// Resolve a run: the paragraph style, the character style the run
    /// names (looked up with `char_style`), then the run's own sprms.
    pub(super) fn resolve<'g>(
        paragraph: CharStyle,
        char_style: impl Fn(u16) -> Option<CharStyle>,
        groups: impl Iterator<Item = &'g [u8]> + Clone,
    ) -> CharProps {
        let named = groups
            .clone()
            .flat_map(sprms)
            .filter(|s| s.op == sprm::C_ISTD)
            .last()
            .and_then(|s| char_style(s.u16()))
            .unwrap_or_default();
        let style = |own: Option<bool>, para: Option<bool>| own.or(para);
        let bold = style(named.bold, paragraph.bold);
        let italic = style(named.italic, paragraph.italic);
        let strike = style(named.strike, paragraph.strike);
        let vanish = style(named.vanish, paragraph.vanish);
        // Direct values; `None` when the run defers to its styles.
        let (mut d_bold, mut d_italic, mut d_strike) = (None, None, None);
        let mut props = CharProps {
            hidden: vanish.unwrap_or(false),
            font: named.font.or(paragraph.font),
            ..CharProps::default()
        };
        let mut iss = named.iss;
        for s in groups.flat_map(sprms) {
            match s.op {
                sprm::C_BOLD => d_bold = direct(s.u8(), bold, named.bold),
                sprm::C_ITALIC => d_italic = direct(s.u8(), italic, named.italic),
                sprm::C_STRIKE | sprm::C_DSTRIKE => d_strike = direct(s.u8(), strike, named.strike),
                sprm::C_VANISH => props.hidden = toggle(s.u8(), vanish),
                sprm::C_ISS => iss = Some(s.u8()),
                sprm::C_RMARK_DEL => props.deleted = s.flag(),
                sprm::C_RMARK_INS => props.inserted = s.flag(),
                sprm::C_SPEC => props.special = s.flag(),
                sprm::C_OLE2 => props.ole = s.flag(),
                sprm::C_DATA => props.data = s.flag(),
                sprm::C_PIC_LOCATION => props.pic = Some(s.u32()),
                sprm::C_SYMBOL => props.symbol = Some((s.u16(), u16_at(s.arg, 2).unwrap_or(0))),
                sprm::C_FONT => props.font = Some(s.u16()),
                _ => {}
            }
        }
        props.bold = d_bold.or(named.bold).unwrap_or(false);
        props.italic = d_italic.or(named.italic).unwrap_or(false);
        props.strike = d_strike.or(named.strike).unwrap_or(false);
        props.superscript = iss == Some(1);
        props.subscript = iss == Some(2);
        props
    }
}

/// A direct toggle, `None` when it only repeats what the paragraph style
/// says.
fn direct(value: u8, style: Option<bool>, char_style: Option<bool>) -> Option<bool> {
    match value {
        0x80 if char_style.is_none() => None,
        _ => Some(toggle(value, style)),
    }
}

/// Paragraph formatting that a style sets.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ParaStyle {
    pub(super) ilfo: Option<u16>,
    pub(super) ilvl: Option<u8>,
    pub(super) outline: Option<u8>,
}

impl ParaStyle {
    pub(super) fn apply(&mut self, grpprl: &[u8]) {
        for s in sprms(grpprl) {
            match s.op {
                sprm::P_ILFO => self.ilfo = Some(s.u16()),
                sprm::P_ILVL => self.ilvl = Some(s.u8()),
                sprm::P_OUT_LVL => self.outline = Some(s.u8()),
                _ => {}
            }
        }
    }
}

/// The properties of a paragraph.
#[derive(Debug, Clone, Default)]
pub(super) struct ParaProps {
    pub(super) istd: u16,
    /// List and outline level set on the paragraph itself.
    pub(super) own: ParaStyle,
    pub(super) in_table: bool,
    /// The paragraph ends a table row.
    pub(super) ttp: bool,
    /// Table depth, when set.
    pub(super) itap: Option<i32>,
    pub(super) inner_cell: bool,
    pub(super) inner_ttp: bool,
    /// The row a table-terminating paragraph ends.
    pub(super) row: Option<RowDef>,
}

impl ParaProps {
    pub(super) fn read<'g>(istd: u16, groups: impl Iterator<Item = &'g [u8]>) -> ParaProps {
        let mut props = ParaProps {
            istd,
            ..ParaProps::default()
        };
        let mut row = RowDef::default();
        let mut has_row = false;
        for s in groups.flat_map(sprms) {
            match s.op {
                sprm::P_ISTD => props.istd = s.u16(),
                sprm::P_ILFO => props.own.ilfo = Some(s.u16()),
                sprm::P_ILVL => props.own.ilvl = Some(s.u8()),
                sprm::P_OUT_LVL => props.own.outline = Some(s.u8()),
                sprm::P_IN_TABLE => props.in_table = s.flag(),
                sprm::P_TTP => props.ttp = s.flag(),
                sprm::P_ITAP => props.itap = Some(s.u32() as i32),
                sprm::P_DTAP => {
                    props.itap = Some(props.itap.unwrap_or(0).saturating_add(s.u32() as i32))
                }
                sprm::P_INNER_TABLE_CELL => props.inner_cell = s.flag(),
                sprm::P_INNER_TTP => props.inner_ttp = s.flag(),
                sprm::T_DEF_TABLE => has_row |= row.define(s.arg).is_some(),
                sprm::T_TABLE_HEADER => row.header = s.flag(),
                sprm::T_VERT_MERGE => {
                    if let (Some(&cell), Some(&flags)) = (s.arg.first(), s.arg.get(1)) {
                        row.vert_merge(usize::from(cell), flags);
                    }
                }
                _ => {}
            }
        }
        props.row = has_row.then_some(row);
        props
    }

    /// Nesting depth: 0 outside tables.
    pub(super) fn depth(&self) -> usize {
        if !self.in_table && self.itap.is_none_or(|d| d <= 0) {
            return 0;
        }
        self.itap.map_or(1, |d| d.clamp(1, 64) as usize)
    }
}

/// How a cell takes part in a vertical merge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum VMerge {
    #[default]
    None,
    Restart,
    Continue,
}

/// One cell of a row definition.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct CellDef {
    /// Left and right edges, in twips.
    pub(super) left: i16,
    pub(super) right: i16,
    /// Merged into the cell before it (legacy horizontal merge).
    pub(super) h_merged: bool,
    pub(super) v_merge: VMerge,
}

/// A table row definition (`sprmTDefTable` and friends).
#[derive(Debug, Clone, Default)]
pub(super) struct RowDef {
    pub(super) cells: Vec<CellDef>,
    pub(super) header: bool,
}

impl RowDef {
    fn define(&mut self, arg: &[u8]) -> Option<()> {
        let count = usize::from(*arg.first()?).min(63);
        let edges: Vec<i16> = (0..=count).map_while(|i| i16_at(arg, 1 + i * 2)).collect();
        if edges.len() != count + 1 {
            return None;
        }
        let tc_start = 1 + (count + 1) * 2;
        self.cells = (0..count)
            .map(|i| {
                let flags = u16_at(arg, tc_start + i * 20).unwrap_or(0);
                CellDef {
                    left: edges[i],
                    right: edges[i + 1],
                    h_merged: flags & 0x2 != 0,
                    v_merge: match (flags & 0x20 != 0, flags & 0x40 != 0) {
                        (true, true) => VMerge::Restart,
                        (true, false) => VMerge::Continue,
                        _ => VMerge::None,
                    },
                }
            })
            .collect();
        Some(())
    }

    fn vert_merge(&mut self, cell: usize, flags: u8) {
        if let Some(def) = self.cells.get_mut(cell) {
            def.v_merge = match flags {
                1 => VMerge::Continue,
                3 => VMerge::Restart,
                _ => VMerge::None,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprm_operands_have_their_sizes() {
        let mut grpprl = vec![0x35, 0x08, 0x81]; // bold: opposite of style
        grpprl.extend([0x03, 0x6A, 1, 2, 3, 4]); // picture location
        grpprl.extend([0x08, 0xD6, 6, 0, 1, 0, 0, 0x10, 0]); // one-cell row
        grpprl.extend([0x15, 0xC6, 2, 0xAA, 0xBB]); // tab changes
        grpprl.extend([0x4F, 0x4A]); // truncated
        let all: Vec<Sprm<'_>> = sprms(&grpprl).collect();
        assert_eq!(all.len(), 4);
        assert_eq!(all[1].u32(), 0x0403_0201);
        assert_eq!(all[2].arg, &[1, 0, 0, 0x10, 0]);
        assert_eq!(all[3].arg, &[0xAA, 0xBB]);
    }

    #[test]
    fn toggles_follow_the_styles() {
        let heading = CharStyle {
            bold: Some(true),
            ..CharStyle::default()
        };
        let none = |_| None;
        // Bold from the paragraph style alone is not shown...
        let props = CharProps::resolve(heading, none, [&[][..]].into_iter());
        assert!(!props.bold);
        // ...but "opposite of the style" turns it off and on correctly.
        let props = CharProps::resolve(heading, none, [&[0x35, 0x08, 0x81][..]].into_iter());
        assert!(!props.bold);
        let props = CharProps::resolve(
            CharStyle::default(),
            none,
            [&[0x35, 0x08, 0x81][..]].into_iter(),
        );
        assert!(props.bold);
        // A character style's bold is shown.
        let strong = |istd| (istd == 5).then_some(heading);
        let props = CharProps::resolve(
            CharStyle::default(),
            strong,
            [&[0x30, 0x4A, 5, 0][..]].into_iter(),
        );
        assert!(props.bold);
    }

    #[test]
    fn row_definitions_read_edges_and_merges() {
        let mut arg = vec![2];
        for edge in [0i16, 1000, 2000] {
            arg.extend(edge.to_le_bytes());
        }
        let mut tc = [0u8; 40];
        tc[0] = 0x60;
        tc[20] = 0x20;
        arg.extend(tc);
        let mut row = RowDef::default();
        row.define(&arg).unwrap();
        assert_eq!(row.cells.len(), 2);
        assert_eq!((row.cells[1].left, row.cells[1].right), (1000, 2000));
        assert_eq!(row.cells[0].v_merge, VMerge::Restart);
        assert_eq!(row.cells[1].v_merge, VMerge::Continue);
        row.vert_merge(1, 0);
        assert_eq!(row.cells[1].v_merge, VMerge::None);
    }
}
