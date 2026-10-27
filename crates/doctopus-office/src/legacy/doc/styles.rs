//! The style sheet, the font table and the list tables of the table
//! stream.

use super::props::{CharStyle, ParaStyle};
use crate::binary::{Cursor, u16_at, u32_at};
use crate::fonts::is_monospace;
use crate::numbering::{CounterStyle, Instance, Level, Numbering};
use crate::ole::text::utf16le;
use crate::word::{Role, role_by_name};

/// How far `istdBase` chains are followed; the limit guards against cycles.
const MAX_INHERITANCE: usize = 16;
/// `istdBase` of a style based on nothing.
const NO_BASE: u16 = 0x0FFF;
/// Built-in style identifiers (`sti`) of the headings and the title.
const STI_HEADING_9: u16 = 9;
const STI_TITLE: u16 = 62;

/// One style definition.
#[derive(Debug, Clone, Default)]
struct Std<'t> {
    name: String,
    sti: u16,
    base: u16,
    papx: &'t [u8],
    chpx: &'t [u8],
}

/// The style sheet, with each style's inherited formatting worked out.
#[derive(Debug, Default)]
pub(super) struct Styles<'t> {
    styles: Vec<Option<Std<'t>>>,
    chars: Vec<CharStyle>,
    paras: Vec<ParaStyle>,
    /// The document's default font.
    pub(super) default_font: Option<u16>,
}

impl<'t> Styles<'t> {
    /// Read the `STSH`.
    pub(super) fn read(stsh: &'t [u8]) -> Styles<'t> {
        let mut out = Styles::default();
        let mut r = Cursor::new(stsh);
        let Some(header) = r.u16().and_then(|cb| r.take(usize::from(cb))) else {
            return out;
        };
        let count = u16_at(header, 0).unwrap_or(0);
        let base_size = usize::from(u16_at(header, 2).unwrap_or(10));
        out.default_font = u16_at(header, 12);
        for _ in 0..count {
            let Some(std) = r.u16().and_then(|cb| r.take(usize::from(cb))) else {
                break;
            };
            out.styles.push(read_std(std, base_size));
        }
        out.resolve();
        out
    }

    /// Work out each style's formatting from its chain of base styles.
    fn resolve(&mut self) {
        let count = self.styles.len();
        self.chars = Vec::with_capacity(count);
        self.paras = Vec::with_capacity(count);
        for istd in 0..count {
            let mut chain = Vec::new();
            let mut current = Some(istd);
            while let Some(i) = current {
                if chain.len() >= MAX_INHERITANCE || chain.contains(&i) {
                    break;
                }
                let Some(Some(std)) = self.styles.get(i) else {
                    break;
                };
                chain.push(i);
                current = (std.base != NO_BASE).then_some(usize::from(std.base));
            }
            let mut chars = CharStyle::default();
            let mut paras = ParaStyle::default();
            for &i in chain.iter().rev() {
                if let Some(Some(std)) = self.styles.get(i) {
                    chars.apply(std.chpx);
                    paras.apply(std.papx);
                }
            }
            self.chars.push(chars);
            self.paras.push(paras);
        }
    }

    /// The character formatting a style sets, inherited formatting
    /// included.
    pub(super) fn chars(&self, istd: u16) -> Option<CharStyle> {
        self.chars.get(usize::from(istd)).copied()
    }

    /// The paragraph formatting a style sets, inherited formatting
    /// included.
    pub(super) fn paras(&self, istd: u16) -> ParaStyle {
        self.paras
            .get(usize::from(istd))
            .copied()
            .unwrap_or_default()
    }

    /// Whether a paragraph style holds code or quotations, by its name or
    /// the name of a style it is based on.
    pub(super) fn role(&self, istd: u16) -> Role {
        let mut current = istd;
        for _ in 0..MAX_INHERITANCE {
            let Some(Some(std)) = self.styles.get(usize::from(current)) else {
                break;
            };
            if let Some(role) = role_by_name(&std.name) {
                return role;
            }
            if std.base == NO_BASE {
                break;
            }
            current = std.base;
        }
        Role::Text
    }

    /// Whether a font, or the default font for `None`, is fixed-width.
    pub(super) fn is_mono(&self, font: Option<u16>, fonts: &[String]) -> bool {
        font.or(self.default_font)
            .and_then(|i| fonts.get(usize::from(i)))
            .is_some_and(|name| is_monospace(name))
    }

    /// The heading level a paragraph style gives, if it is a heading: by
    /// its built-in identity or name, else by its outline level.
    pub(super) fn heading_level(&self, istd: u16) -> Option<u8> {
        let mut current = istd;
        for _ in 0..MAX_INHERITANCE {
            let std = self.styles.get(usize::from(current))?.as_ref()?;
            let name = std.name.to_ascii_lowercase();
            if (1..=STI_HEADING_9).contains(&std.sti) {
                return Some(std.sti as u8);
            }
            if let Some(n) = name
                .strip_prefix("heading ")
                .and_then(|n| n.trim().parse::<u8>().ok())
            {
                return Some(n.clamp(1, 9));
            }
            if std.sti == STI_TITLE || name == "title" {
                return Some(1);
            }
            let mut own = ParaStyle::default();
            own.apply(std.papx);
            if let Some(outline) = own.outline {
                return (outline < 9).then_some(outline + 1);
            }
            if std.base == NO_BASE {
                return None;
            }
            current = std.base;
        }
        None
    }
}

fn read_std(data: &[u8], base_size: usize) -> Option<Std<'_>> {
    let sti = u16_at(data, 0)? & 0x0FFF;
    let kind_base = u16_at(data, 2)?;
    let kind = kind_base & 0xF;
    let base = kind_base >> 4;
    let upx_count = usize::from(u16_at(data, 4)? & 0xF);
    let mut r = Cursor::at(data, base_size);
    let len = usize::from(r.u16()?);
    let name = utf16le(r.take(len * 2)?);
    r.skip(2)?;
    let mut upx: Vec<&[u8]> = Vec::with_capacity(upx_count);
    for _ in 0..upx_count {
        let Some(cb) = r.u16().map(usize::from) else {
            break;
        };
        let Some(bytes) = r.take(cb) else {
            break;
        };
        upx.push(bytes);
        if cb % 2 == 1 {
            r.skip(1);
        }
    }
    let at = |i: usize| upx.get(i).copied().unwrap_or_default();
    let (papx, chpx) = match kind {
        1 => (papx(at(0)), at(1)),
        2 => (&[][..], at(0)),
        3 => (papx(at(1)), at(2)),
        4 => (papx(at(0)), &[][..]),
        _ => (&[][..], &[][..]),
    };
    Some(Std {
        name,
        sti,
        base,
        papx,
        chpx,
    })
}

/// Paragraph style properties start with the style index.
fn papx(upx: &[u8]) -> &[u8] {
    upx.get(2..).unwrap_or_default()
}

/// Font names by index, from the `SttbfFfn`.
pub(super) fn fonts(table: &[u8]) -> Vec<String> {
    let mut r = Cursor::new(table);
    let Some(count) = r.u16() else {
        return Vec::new();
    };
    if r.skip(2).is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for _ in 0..count {
        let Some(ffn) = r.u8().and_then(|len| r.take(usize::from(len))) else {
            break;
        };
        // The name follows 39 bytes of font metrics.
        out.push(ffn.get(39..).map(utf16le).unwrap_or_default());
    }
    out
}

/// The list definitions (`PlfLst`, followed by the levels) and list
/// instances (`PlfLfo`), as numbering ids: definitions by list id,
/// instances by 1-based instance index.
pub(super) fn numbering(lists: Option<&[u8]>, instances: Option<&[u8]>) -> Numbering {
    let mut numbering = Numbering::default();
    if let Some(data) = lists {
        read_lists(data, &mut numbering);
    }
    if let Some(data) = instances {
        read_instances(data, &mut numbering);
    }
    numbering
}

fn read_lists(data: &[u8], numbering: &mut Numbering) -> Option<()> {
    let mut r = Cursor::new(data);
    let count = usize::from(r.u16()?);
    let mut lists = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let lstf = r.take(28)?;
        let simple = lstf[26] & 0x01 != 0;
        lists.push((u32_at(lstf, 0)?, simple));
    }
    // The levels of every list follow the list table.
    for (id, simple) in lists {
        let count = if simple { 1 } else { 9 };
        let mut levels = vec![Level::default(); 9];
        for slot in levels.iter_mut().take(count) {
            *slot = read_level(&mut r)?;
        }
        numbering.abstracts.insert(id.to_string(), levels);
    }
    Some(())
}

fn read_instances(data: &[u8], numbering: &mut Numbering) -> Option<()> {
    let mut r = Cursor::new(data);
    let count = r.u32()? as usize;
    let mut lfos = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        let lfo = r.take(16)?;
        lfos.push((u32_at(lfo, 0)?, lfo[12]));
    }
    for (i, (list, overrides)) in lfos.into_iter().enumerate() {
        let mut instance = Instance {
            abstract_id: list.to_string(),
            ..Instance::default()
        };
        // Overrides follow all instances, one group per instance; a
        // damaged group still leaves the instance itself.
        if r.skip(4).is_some() {
            for _ in 0..overrides {
                let (Some(start), Some(flags)) = (r.i32(), r.u32()) else {
                    break;
                };
                let level = (flags & 0xF).min(8) as u8;
                if flags & 0x10 != 0 {
                    instance.start_overrides.insert(level, start.max(0) as u32);
                }
                if flags & 0x20 != 0 {
                    match read_level(&mut r) {
                        Some(lvl) => {
                            instance.level_overrides.insert(level, lvl);
                        }
                        None => break,
                    }
                }
            }
        }
        numbering.instances.insert((i + 1).to_string(), instance);
    }
    Some(())
}

/// One list level (`LVL`): its counter and the label template, with the
/// placeholders turned into `%1`..`%9`.
fn read_level(r: &mut Cursor<'_>) -> Option<Level> {
    let lvlf = r.take(28)?;
    let start = u32_at(lvlf, 0)? as i32;
    let style = CounterStyle::from_nfc(lvlf[4]);
    let placeholders = &lvlf[6..15];
    let chpx = usize::from(lvlf[24]);
    let papx = usize::from(lvlf[25]);
    r.skip(papx + chpx)?;
    let len = usize::from(r.u16()?);
    let units = r.take(len * 2)?;
    let mut text = String::new();
    for (k, unit) in units.as_chunks::<2>().0.iter().enumerate() {
        let unit = u16::from_le_bytes(*unit);
        let position = u8::try_from(k + 1).unwrap_or(0);
        if unit < 9 && placeholders.contains(&position) {
            text.push_str(&format!("%{}", unit + 1));
        } else if let Some(c) = char::from_u32(u32::from(unit)) {
            text.push(c);
        }
    }
    Some(Level {
        start: start.max(0) as u32,
        style,
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::numbering::{Counters, ListLabel, NumRef};

    fn level(start: i32, nfc: u8, xst: &[u16], placeholders: &[u8]) -> Vec<u8> {
        let mut out = start.to_le_bytes().to_vec();
        out.push(nfc);
        out.push(0);
        let mut nums = [0u8; 9];
        nums[..placeholders.len()].copy_from_slice(placeholders);
        out.extend(nums);
        out.extend([0; 9]);
        out.extend([0, 0, 0, 0]);
        out.extend((xst.len() as u16).to_le_bytes());
        out.extend(xst.iter().flat_map(|u| u.to_le_bytes()));
        out
    }

    #[test]
    fn lists_become_numbering_definitions() {
        let mut lists = 1u16.to_le_bytes().to_vec();
        let mut lstf = [0u8; 28];
        lstf[0..4].copy_from_slice(&77u32.to_le_bytes());
        lstf[26] = 0x01;
        lists.extend(lstf);
        // "%1.%2)" is not possible in a simple list; "(%1)" is.
        lists.extend(level(3, 4, &[b'(' as u16, 0, b')' as u16], &[2]));

        let mut lfos = 1u32.to_le_bytes().to_vec();
        let mut lfo = [0u8; 16];
        lfo[0..4].copy_from_slice(&77u32.to_le_bytes());
        lfos.extend(lfo);
        lfos.extend([0xFF; 4]);

        let numbering = numbering(Some(&lists), Some(&lfos));
        let mut counters = Counters::default();
        let num = NumRef {
            num_id: "1".into(),
            level: Some(0),
        };
        assert_eq!(
            counters.next(&numbering, &num),
            ListLabel::Number("(c)".into())
        );
        assert_eq!(
            counters.next(&numbering, &num),
            ListLabel::Number("(d)".into())
        );
    }

    #[test]
    fn styles_inherit_and_name_headings() {
        fn std(sti: u16, kind: u16, base: u16, name: &str, chpx: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend(sti.to_le_bytes());
            out.extend((kind | base << 4).to_le_bytes());
            out.extend(2u16.to_le_bytes());
            out.extend([0; 4]);
            out.extend((name.len() as u16).to_le_bytes());
            out.extend(name.encode_utf16().flat_map(u16::to_le_bytes));
            out.extend([0, 0]);
            out.extend(2u16.to_le_bytes());
            out.extend([0, 0]);
            out.extend((chpx.len() as u16).to_le_bytes());
            out.extend(chpx);
            if chpx.len() % 2 == 1 {
                out.push(0);
            }
            let mut lp = (out.len() as u16).to_le_bytes().to_vec();
            lp.extend(out);
            lp
        }
        let mut stsh = 18u16.to_le_bytes().to_vec();
        let mut header = [0u8; 18];
        header[0..2].copy_from_slice(&3u16.to_le_bytes());
        header[2..4].copy_from_slice(&10u16.to_le_bytes());
        stsh.extend(header);
        stsh.extend(std(0, 1, NO_BASE, "Normal", &[]));
        stsh.extend(std(1, 1, 0, "heading 1", &[0x35, 0x08, 1]));
        stsh.extend(std(0x0FFE, 1, 1, "My Heading", &[0x36, 0x08, 1]));

        let styles = Styles::read(&stsh);
        assert_eq!(styles.heading_level(1), Some(1));
        assert_eq!(styles.heading_level(2), Some(1));
        assert_eq!(styles.heading_level(0), None);
        let mine = styles.chars(2).unwrap();
        assert_eq!((mine.bold, mine.italic), (Some(true), Some(true)));
    }

    #[test]
    fn font_names_follow_the_metrics() {
        let mut ffn = vec![0u8; 39];
        ffn.extend("Symbol\0".encode_utf16().flat_map(u16::to_le_bytes));
        let mut table = 1u16.to_le_bytes().to_vec();
        table.extend([0, 0]);
        table.push(ffn.len() as u8);
        table.extend(ffn);
        assert_eq!(fonts(&table), ["Symbol"]);
    }
}
