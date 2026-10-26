//! Excel 97-2003 workbooks (BIFF8, `.xls`; BIFF5 files of Excel 95 for
//! their cells).
//!
//! Cell values, formulas and merged ranges come from calamine. The workbook
//! stream is also walked here for what calamine does not read: cell notes,
//! hyperlinks, page headers and footers, and the drawing layer with its
//! pictures, text boxes and embedded objects.

use std::collections::{HashMap, HashSet};
use std::io::Cursor as IoCursor;

use calamine::{Reader as _, SheetType, SheetVisible, Xls, XlsError};
use doctopus_core::{
    Block, CellRef, Document, EmbedRef, Error, Group, GroupKind, ImageRef, Input, Result, Section,
    SectionKind,
};

use super::context::Context;
use super::officeart::{self, Blip, BlipStore, Records, Shape};
use crate::binary::{Cursor, u16_at, u32_at};
use crate::blocks::{link_shape, push_text_box};
use crate::ole::hyperlink;
use crate::ole::text::{ansi, utf16le};
use crate::sheet::{self, MAX_CELLS, SheetCells, header_footer_text};
use doctopus_core::container::compound_file;

/// Record types.
mod rt {
    pub(super) const BOF: u16 = 0x0809;
    pub(super) const EOF: u16 = 0x000A;
    pub(super) const FILEPASS: u16 = 0x002F;
    pub(super) const BOUNDSHEET: u16 = 0x0085;
    pub(super) const CONTINUE: u16 = 0x003C;
    pub(super) const HEADER: u16 = 0x0014;
    pub(super) const FOOTER: u16 = 0x0015;
    pub(super) const NOTE: u16 = 0x001C;
    pub(super) const OBJ: u16 = 0x005D;
    pub(super) const TXO: u16 = 0x01B6;
    pub(super) const HLINK: u16 = 0x01B8;
    pub(super) const MSODRAWINGGROUP: u16 = 0x00EB;
    pub(super) const MSODRAWING: u16 = 0x00EC;
    pub(super) const IMDATA: u16 = 0x007F;
    pub(super) const DIMENSIONS: u16 = 0x0200;
    pub(super) const FORMULA: u16 = 0x0006;
    pub(super) const STRING: u16 = 0x0207;
    pub(super) const NUMBER: u16 = 0x0203;
    pub(super) const LABEL: u16 = 0x0204;
    pub(super) const BOOLERR: u16 = 0x0205;
    pub(super) const RK: u16 = 0x027E;
    pub(super) const RSTRING: u16 = 0x00D6;
    pub(super) const LABELSST: u16 = 0x00FD;
    pub(super) const MULRK: u16 = 0x00BD;
}

/// Object types (`ftCmo.ot`).
const OT_CHART: u16 = 0x05;
const OT_COMMENT: u16 = 0x19;

/// Grid positions calamine may lay out per sheet. It reads each sheet into
/// a dense grid from the first cell to the last when it opens a workbook,
/// so two cells far apart would otherwise cost gigabytes.
const MAX_GRID: u64 = 2 * MAX_CELLS as u64;
/// Columns of a sheet: `IV` is the last.
const COLUMNS: u32 = 256;
/// What records calamine should skip are retyped to: no BIFF record has it.
const SKIPPED: u16 = 0x0000;

/// Cells a single hyperlink is attached to at most.
const MAX_LINK_CELLS: u32 = 1_000;

/// Read an `.xls` workbook.
pub(crate) fn read(input: &Input<'_>) -> Result<Document> {
    let mut cx = Context::open(input)?;
    cx.detect_macros(&["/_VBA_PROJECT_CUR"]);
    let path = if cx.cfb.is_stream("/Workbook") {
        "/Workbook"
    } else {
        "/Book"
    };
    let stream = cx.required_stream(path)?;
    let globals = Globals::read(&stream);
    if globals.encrypted {
        return Err(Error::encrypted(input.format.name));
    }
    let store = officeart::Records::new(&globals.drawing_group)
        .next()
        .map(|dgg| BlipStore::from_drawing_group(&dgg, cx.options.max_part_size))
        .unwrap_or_default();
    let bounded = bound_cells(&stream, &globals.sheets);
    let repacked;
    let workbook = match &bounded.stream {
        Some(stream) => {
            repacked = compound_file(&[(path, stream)])
                .map_err(|e| Error::malformed(input.format.name, e.to_string()))?;
            repacked.as_slice()
        }
        None => input.data,
    };
    if bounded.stray > 0 {
        cx.doc.warn(match bounded.stray {
            1 => "1 cell beyond the last column was skipped".to_string(),
            n => format!("{n} cells beyond the last column were skipped"),
        });
    }
    let mut book = Xls::new(IoCursor::new(workbook)).map_err(|e| match e {
        XlsError::Password => Error::encrypted(input.format.name),
        e => Error::malformed(input.format.name, e.to_string()),
    })?;
    let mut used_storages = HashSet::new();
    let mut charts = 0;
    let mut sections = Vec::new();
    let mut number = 0;
    for meta in book.sheets_metadata().to_vec() {
        if meta.typ == SheetType::Vba {
            continue;
        }
        number += 1;
        let mut section = Section::new(SectionKind::Sheet)
            .with_number(number)
            .with_name(meta.name.clone());
        match meta.visible {
            SheetVisible::Visible => {}
            SheetVisible::Hidden => section.hidden = true,
            SheetVisible::VeryHidden => {
                section.hidden = true;
                section
                    .properties
                    .insert("visibility".into(), "very hidden".into());
            }
        }
        let extras = globals
            .sheets
            .iter()
            .find(|(name, _)| *name == meta.name)
            .map(|(_, offset)| SheetExtras::read(&stream, *offset, globals.biff8))
            .unwrap_or_default();
        if meta.typ == SheetType::ChartSheet {
            section
                .properties
                .insert("type".into(), "chart sheet".into());
            charts += 1;
            sections.push(section);
            continue;
        }
        let mut cells = SheetCells::default();
        match book.worksheet_range(&meta.name) {
            Ok(range) => cells.add_values(&range),
            Err(e) => cx.doc.warn(format!(
                "cells of sheet \"{}\" could not be read: {e}",
                meta.name
            )),
        }
        if cx.options.formulas
            && let Ok(range) = book.worksheet_formula(&meta.name)
        {
            cells.add_formulas(&range);
        }
        let cut = globals
            .sheets
            .iter()
            .find(|(name, _)| *name == meta.name)
            .and_then(|(_, offset)| bounded.cut.get(offset));
        if let Some(row) = cut {
            cx.doc.warn(format!(
                "sheet \"{}\" was cut off after row {row}: its cells are spread over more than {MAX_GRID} positions",
                meta.name
            ));
        }
        cells.merged = book
            .merge_cells_by_sheet_name(&meta.name)
            .unwrap_or_default();
        cells.links = extras.links.clone();
        cells.comments = extras.notes();
        let built = sheet::build(cells);
        if built.truncated {
            cx.doc.warn(format!(
                "sheet \"{}\" has more than {MAX_CELLS} cells; only the first {MAX_CELLS} were read",
                meta.name
            ));
        }
        if !built.grid.rows.is_empty() {
            section.blocks.push(Block::Grid(built.grid));
        }
        let mut drawing = Drawing {
            cx: &mut cx,
            store: &store,
            sheet: &meta.name,
            objects: &extras.objects,
            used_storages: &mut used_storages,
            charts: 0,
        };
        section.blocks.extend(drawing.blocks(&extras.drawing));
        charts += drawing.charts;
        for (n, (format, data)) in extras.pictures.iter().enumerate() {
            if let Some(blip) = picture_data(*format, data) {
                let origin = format!("imdata:{}:{n}", meta.name);
                let location = format!("sheet \"{}\"", meta.name);
                let part = cx.picture(blip, origin, &location);
                section
                    .blocks
                    .push(Block::Image(ImageRef { part, alt: None }));
            }
        }
        for (kind, text) in [
            (GroupKind::Header, &extras.header),
            (GroupKind::Footer, &extras.footer),
        ] {
            let text = text.as_deref().map(header_footer_text).unwrap_or_default();
            if !text.is_empty() {
                section.blocks.push(Block::Group(Group {
                    kind,
                    title: None,
                    blocks: vec![Block::text(text)],
                }));
            }
        }
        sections.push(section);
    }
    // Embedded objects no drawing refers to.
    let orphans: Vec<String> = cx
        .cfb
        .list("/")
        .into_iter()
        .filter(|e| !e.is_stream && e.name.starts_with("MBD") && !used_storages.contains(&e.path))
        .map(|e| e.path)
        .collect();
    for path in orphans {
        if let Some(part) = cx.storage_object(&path, None, "workbook")
            && let Some(first) = sections.first_mut()
        {
            first.blocks.push(Block::Embed(EmbedRef { part }));
        }
    }
    if charts > 0 {
        cx.doc.warn(format!(
            "{charts} chart(s) not shown: charts in .xls files are not read; save the workbook as .xlsx to include them"
        ));
    }
    cx.doc.sections = sections;
    Ok(cx.doc)
}

// ----- the BIFF stream -------------------------------------------------------

/// One BIFF record.
struct Record<'a> {
    /// Where its header starts in the stream.
    at: usize,
    kind: u16,
    data: &'a [u8],
}

/// Records from `offset` on; stops at a header that does not fit.
fn records(stream: &[u8], offset: usize) -> impl Iterator<Item = Record<'_>> {
    let mut pos = offset;
    std::iter::from_fn(move || {
        let at = pos;
        let mut r = Cursor::at(stream, pos);
        let kind = r.u16()?;
        let len = r.u16()? as usize;
        let data = r.take(len.min(r.remaining()))?;
        pos = r.pos();
        Some(Record { at, kind, data })
    })
}

/// The records of a sheet's substream, without those of substreams nested
/// in it (embedded charts).
fn sheet_records(stream: &[u8], offset: usize) -> impl Iterator<Item = Record<'_>> {
    let mut depth = 0usize;
    let mut started = false;
    records(stream, offset)
        .take_while(move |r| match r.kind {
            rt::BOF if started => {
                depth += 1;
                true
            }
            rt::BOF => {
                started = true;
                true
            }
            rt::EOF if depth > 0 => {
                depth -= 1;
                true
            }
            rt::EOF => false,
            _ => true,
        })
        .filter(move |r| depth == 0 && r.kind != rt::EOF)
}

/// What [`bound_cells`] changed.
#[derive(Default)]
struct Bounded {
    /// The workbook stream with the records calamine should skip retyped,
    /// when there were any.
    stream: Option<Vec<u8>>,
    /// Sheets cut off: substream offset → last row kept (1-based).
    cut: HashMap<usize, u32>,
    /// Cells left out for lying beyond the last column.
    stray: usize,
}

/// Keep the grids calamine lays out small (see [`MAX_GRID`]): cells
/// beyond the last column, and rows past what fits, are retyped so that
/// calamine skips them; so are sheet dimensions claiming more than fits.
/// Record lengths stay the same, so stream offsets still hold.
fn bound_cells(stream: &[u8], sheets: &[(String, usize)]) -> Bounded {
    let mut out = Bounded::default();
    let skip = |at: usize, out: &mut Bounded| {
        let patched = out.stream.get_or_insert_with(|| stream.to_vec());
        patched[at..at + 2].copy_from_slice(&SKIPPED.to_le_bytes());
    };
    for &(_, offset) in sheets {
        // The area the cells within the columns cover.
        let mut area: Option<(u32, u32, u32, u32)> = None;
        for record in sheet_records(stream, offset) {
            if let Some((row, first, last)) = cell_span(&record)
                && last < COLUMNS
            {
                let (r0, r1, c0, c1) = area.get_or_insert((row, row, first, last));
                (*r0, *r1) = ((*r0).min(row), (*r1).max(row));
                (*c0, *c1) = ((*c0).min(first), (*c1).max(last));
            }
        }
        let rows_kept = area.and_then(|(r0, r1, c0, c1)| {
            let columns = u64::from(c1 - c0 + 1);
            let rows = u64::from(r1 - r0 + 1);
            (rows * columns > MAX_GRID).then(|| r0 + (MAX_GRID / columns) as u32)
        });
        if let Some(end) = rows_kept {
            out.cut.insert(offset, end);
        }
        // A formula's string result follows it; it goes with it.
        let mut formula_skipped = false;
        for record in sheet_records(stream, offset) {
            match record.kind {
                rt::DIMENSIONS if dimensions_too_large(record.data) => skip(record.at, &mut out),
                rt::STRING if formula_skipped => skip(record.at, &mut out),
                _ => {}
            }
            let Some((row, _, last)) = cell_span(&record) else {
                continue;
            };
            let stray = last >= COLUMNS;
            let skipped = stray || rows_kept.is_some_and(|end| row >= end);
            if skipped {
                skip(record.at, &mut out);
                out.stray += usize::from(stray);
            }
            if record.kind == rt::FORMULA {
                formula_skipped = skipped;
            }
        }
    }
    out
}

/// The row and the first and last columns of a record calamine reads as
/// cells.
fn cell_span(record: &Record<'_>) -> Option<(u32, u32, u32)> {
    let data = record.data;
    let row = u32::from(u16_at(data, 0)?);
    let first = u32::from(u16_at(data, 2)?);
    let last = match record.kind {
        rt::NUMBER
        | rt::LABEL
        | rt::RSTRING
        | rt::BOOLERR
        | rt::RK
        | rt::LABELSST
        | rt::FORMULA => first,
        rt::MULRK => u32::from(u16_at(data, data.len().checked_sub(2)?)?),
        _ => return None,
    };
    Some((row, first, last.max(first)))
}

/// Whether a `DIMENSIONS` record claims more than [`MAX_GRID`] positions,
/// or an area calamine would get wrong (last row before the first).
fn dimensions_too_large(data: &[u8]) -> bool {
    let (first_row, end_row, first_col, end_col) = match data.len() {
        10 => (
            u64::from(u16_at(data, 0).unwrap_or(0)),
            u64::from(u16_at(data, 2).unwrap_or(0)),
            u64::from(u16_at(data, 4).unwrap_or(0)),
            u64::from(u16_at(data, 6).unwrap_or(0)),
        ),
        14 | 16 => (
            u64::from(u32_at(data, 0).unwrap_or(0)),
            u64::from(u32_at(data, 4).unwrap_or(0)),
            u64::from(u16_at(data, 8).unwrap_or(0)),
            u64::from(u16_at(data, 10).unwrap_or(0)),
        ),
        _ => return false,
    };
    // Calamine reads a first column past the last one as 0.
    let first_col = if first_col > 0xFF || end_col < first_col {
        0
    } else {
        first_col
    };
    if end_row == 0 || end_col == 0 {
        return false;
    }
    end_row <= first_row || (end_row - first_row) * (end_col - first_col) > MAX_GRID
}

/// What the workbook globals say.
#[derive(Default)]
struct Globals {
    biff8: bool,
    encrypted: bool,
    /// Sheet names and the stream offsets of their substreams.
    sheets: Vec<(String, usize)>,
    /// The drawing group (picture store), CONTINUE records joined.
    drawing_group: Vec<u8>,
}

impl Globals {
    fn read(stream: &[u8]) -> Globals {
        let mut g = Globals::default();
        let mut last = 0;
        for record in records(stream, 0) {
            match record.kind {
                rt::BOF => g.biff8 = Cursor::new(record.data).u16() == Some(0x0600),
                rt::EOF => break,
                rt::FILEPASS => g.encrypted = true,
                rt::BOUNDSHEET => {
                    let mut r = Cursor::new(record.data);
                    let parsed = (|| {
                        let offset = r.u32()? as usize;
                        r.skip(2)?;
                        let cch = r.u8()? as usize;
                        let name = if g.biff8 {
                            flagged_string(&mut r, cch)?
                        } else {
                            ansi(r.take(cch)?)
                        };
                        Some((name, offset))
                    })();
                    g.sheets.extend(parsed);
                }
                rt::MSODRAWINGGROUP => g.drawing_group.extend_from_slice(record.data),
                rt::CONTINUE if last == rt::MSODRAWINGGROUP => {
                    g.drawing_group.extend_from_slice(record.data)
                }
                _ => {}
            }
            if record.kind != rt::CONTINUE {
                last = record.kind;
            }
        }
        g
    }
}

/// An `Obj` record: a drawing object of a sheet.
#[derive(Debug, Clone, Default)]
struct Obj {
    id: u16,
    kind: u16,
    /// The embedding storage (`MBDxxxxxxxx`) of an embedded object.
    storage: Option<u32>,
    prog_id: Option<String>,
    /// The text of a text box or comment (from the following `TXO`).
    text: Option<String>,
}

/// What a sheet substream holds besides its cells.
#[derive(Default)]
struct SheetExtras {
    header: Option<String>,
    footer: Option<String>,
    links: HashMap<(u32, u32), String>,
    /// Notes: (row, column, object id, author).
    note_refs: Vec<(u32, u32, u16, String)>,
    /// The drawing (`OfficeArtDgContainer`), MSODRAWING records joined.
    drawing: Vec<u8>,
    objects: Vec<Obj>,
    /// Pictures of Excel 95 files (`IMDATA`): format and data.
    pictures: Vec<(u16, Vec<u8>)>,
    /// Note text of Excel 95 files, which keep it in the note record.
    inline_notes: HashMap<(u32, u32), String>,
}

impl SheetExtras {
    fn read(stream: &[u8], offset: usize, biff8: bool) -> SheetExtras {
        if biff8 {
            Self::read_biff8(stream, offset)
        } else {
            Self::read_biff5(stream, offset)
        }
    }

    /// Excel 95: headers, footers and notes hold 8-bit text; pictures are
    /// `IMDATA` records.
    fn read_biff5(stream: &[u8], offset: usize) -> SheetExtras {
        let mut x = SheetExtras::default();
        let mut depth = 0usize;
        let mut last_note: Option<(u32, u32)> = None;
        let mut iter = records(stream, offset).peekable();
        while let Some(record) = iter.next() {
            let mut r = Cursor::new(record.data);
            match record.kind {
                rt::BOF => depth += 1,
                rt::EOF => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                _ if depth > 1 => {}
                rt::HEADER | rt::FOOTER => {
                    let text = r.u8().and_then(|n| r.take(usize::from(n))).map(ansi);
                    if record.kind == rt::HEADER {
                        x.header = text;
                    } else {
                        x.footer = text;
                    }
                }
                rt::NOTE => {
                    let Some((row, col, cch)) = (|| Some((r.u16()?, r.u16()?, r.u16()?)))() else {
                        continue;
                    };
                    let text = ansi(
                        r.take(usize::from(cch).min(r.remaining()))
                            .unwrap_or_default(),
                    );
                    // Long notes continue in records with row 0xFFFF.
                    let at = if row == 0xFFFF {
                        last_note
                    } else {
                        Some((u32::from(row), u32::from(col)))
                    };
                    if let Some(at) = at {
                        x.inline_notes
                            .entry(at)
                            .or_default()
                            .push_str(&text.replace('\r', "\n"));
                        last_note = Some(at);
                    }
                }
                rt::IMDATA => {
                    let Some((format, _env, len)) = (|| Some((r.u16()?, r.u16()?, r.u32()?)))()
                    else {
                        continue;
                    };
                    let mut data = r.take(r.remaining()).unwrap_or_default().to_vec();
                    while data.len() < len as usize
                        && iter.peek().is_some_and(|r| r.kind == rt::CONTINUE)
                    {
                        if let Some(next) = iter.next() {
                            data.extend_from_slice(next.data);
                        }
                    }
                    data.truncate(len as usize);
                    x.pictures.push((format, data));
                }
                _ => {}
            }
        }
        x
    }

    fn read_biff8(stream: &[u8], offset: usize) -> SheetExtras {
        let mut x = SheetExtras::default();
        let mut depth = 0usize;
        let mut iter = records(stream, offset).peekable();
        while let Some(record) = iter.next() {
            match record.kind {
                rt::BOF => depth += 1,
                rt::EOF => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                // Embedded chart substreams are skipped.
                _ if depth > 1 => {}
                rt::HEADER => x.header = xl_string(record.data),
                rt::FOOTER => x.footer = xl_string(record.data),
                rt::HLINK => x.hyperlink(record.data),
                rt::NOTE => {
                    let mut r = Cursor::new(record.data);
                    let parsed = (|| {
                        let (row, col) = (r.u16()?, r.u16()?);
                        r.skip(2)?;
                        let id = r.u16()?;
                        let cch = r.u16()? as usize;
                        let author = flagged_string(&mut r, cch).unwrap_or_default();
                        Some((u32::from(row), u32::from(col), id, author))
                    })();
                    x.note_refs.extend(parsed);
                }
                rt::MSODRAWING => x.drawing.extend_from_slice(record.data),
                rt::OBJ => x.objects.push(obj(record.data)),
                rt::TXO => {
                    let cch = Cursor::at(record.data, 10).u16().unwrap_or(0) as usize;
                    let mut text = String::new();
                    let mut left = cch;
                    while left > 0 && iter.peek().is_some_and(|r| r.kind == rt::CONTINUE) {
                        let Some(next) = iter.next() else { break };
                        let mut r = Cursor::new(next.data);
                        let Some(flags) = r.u8() else { break };
                        let wide = flags & 1 == 1;
                        let n = left.min(r.remaining() / if wide { 2 } else { 1 });
                        let Some(raw) = r.take(if wide { n * 2 } else { n }) else {
                            break;
                        };
                        text.push_str(&if wide { utf16le_all(raw) } else { latin1(raw) });
                        left -= n;
                        if n == 0 {
                            break;
                        }
                    }
                    if let Some(last) = x.objects.last_mut() {
                        last.text = Some(text.replace('\r', "\n"));
                    }
                }
                _ => {}
            }
        }
        x
    }

    fn hyperlink(&mut self, data: &[u8]) {
        let mut r = Cursor::new(data);
        let Some((r0, r1, c0, c1)) = (|| Some((r.u16()?, r.u16()?, r.u16()?, r.u16()?)))() else {
            return;
        };
        let Some(target) = data.get(8 + 16..).and_then(hyperlink::parse) else {
            return;
        };
        let mut n = 0;
        'cells: for row in u32::from(r0)..=u32::from(r1.max(r0)) {
            for col in u32::from(c0)..=u32::from(c1.max(c0)) {
                n += 1;
                if n > MAX_LINK_CELLS {
                    break 'cells;
                }
                self.links
                    .entry((row, col))
                    .or_insert_with(|| target.clone());
            }
        }
    }

    /// Note text by cell, prefixed with the author.
    fn notes(&self) -> HashMap<(u32, u32), String> {
        let mut out = self.inline_notes.clone();
        out.extend(self.note_refs.iter().filter_map(|(row, col, id, author)| {
            let text = self
                .objects
                .iter()
                .find(|o| o.id == *id)?
                .text
                .as_deref()?
                .trim();
            let author = author.trim();
            // Excel starts note text with "Author:" itself.
            let entry = if author.is_empty() || text.starts_with(author) {
                text.to_string()
            } else {
                format!("{author}: {text}")
            };
            Some(((*row, *col), entry))
        }));
        out
    }
}

/// Parse an `Obj` record's sub-records: the common data (`ftCmo`) and, for
/// embedded objects, the storage id (`ftPictFmla`).
fn obj(data: &[u8]) -> Obj {
    let mut o = Obj::default();
    let mut r = Cursor::new(data);
    while let (Some(ft), Some(cb)) = (r.u16(), r.u16()) {
        let Some(body) = r.take(usize::from(cb).min(r.remaining())) else {
            break;
        };
        match ft {
            0x0015 => {
                let mut b = Cursor::new(body);
                o.kind = b.u16().unwrap_or_default();
                o.id = b.u16().unwrap_or_default();
            }
            0x0009 => {
                let fmla_len = Cursor::new(body).u16().unwrap_or(0) as usize;
                if body.len() >= 2 + fmla_len + 4 {
                    o.storage = Cursor::at(body, 2 + fmla_len).u32();
                }
                o.prog_id = class_name(body.get(2..2 + fmla_len).unwrap_or_default());
            }
            0x0000 => break,
            _ => {}
        }
        // A comment's ftNts and list boxes' ftLbsData end the useful part.
        if o.kind == OT_COMMENT && ft == 0x000D {
            break;
        }
    }
    o
}

/// The class name stored after the formula of an embedded object.
fn class_name(fmla: &[u8]) -> Option<String> {
    let mut r = Cursor::new(fmla);
    let size = r.u16()? as usize;
    r.skip(4 + size)?;
    if r.u8()? != 0x03 {
        return None;
    }
    let cch = r.u16()? as usize;
    let name = flagged_string(&mut r, cch)?;
    Some(name).filter(|n| !n.trim().is_empty())
}

/// An `XLUnicodeString`: 16-bit length, flags, characters.
fn xl_string(data: &[u8]) -> Option<String> {
    let mut r = Cursor::new(data);
    let cch = r.u16()? as usize;
    flagged_string(&mut r, cch).filter(|s| !s.is_empty())
}

/// Characters after a flags byte: UTF-16 when its low bit is set, otherwise
/// the low bytes of UTF-16 (Latin-1).
fn flagged_string(r: &mut Cursor<'_>, cch: usize) -> Option<String> {
    let flags = r.u8()?;
    Some(if flags & 1 == 1 {
        utf16le_all(r.take(cch * 2)?)
    } else {
        latin1(r.take(cch)?)
    })
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// UTF-16 including any embedded NULs (text boxes are not NUL-terminated).
fn utf16le_all(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let s = String::from_utf16_lossy(&units);
    if s.contains('\0') { utf16le(bytes) } else { s }
}

// ----- drawings --------------------------------------------------------------

struct Drawing<'c, 'a, 'x> {
    cx: &'c mut Context<'a>,
    store: &'x BlipStore,
    sheet: &'x str,
    objects: &'x [Obj],
    used_storages: &'x mut HashSet<String>,
    charts: usize,
}

impl Drawing<'_, '_, '_> {
    /// Blocks for the pictures, text boxes and objects of a sheet. Each
    /// shape is followed by its `Obj` record, so shapes and objects pair up
    /// in order.
    fn blocks(&mut self, data: &[u8]) -> Vec<Block> {
        let Some(dg) = Records::new(data).find(|r| r.kind == officeart::rt::DG_CONTAINER) else {
            return Vec::new();
        };
        let shapes = officeart::shapes(&dg);
        let mut index = 0;
        let mut out = Vec::new();
        for shape in &shapes {
            self.shape(shape, &mut index, &mut out);
        }
        out
    }

    fn shape(&mut self, shape: &Shape<'_>, index: &mut usize, out: &mut Vec<Block>) {
        let obj = self.objects.get(*index).cloned().unwrap_or_default();
        *index += 1;
        if shape.is_deleted() {
            return;
        }
        let location = match shape.client_anchor.and_then(anchor_cell) {
            Some(cell) => format!("sheet \"{}\" cell {cell}", self.sheet),
            None => format!("sheet \"{}\"", self.sheet),
        };
        if !shape.children.is_empty() {
            let mut members = Vec::new();
            for child in &shape.children {
                self.shape(child, index, &mut members);
            }
            out.extend(members);
            return;
        }
        match obj.kind {
            OT_COMMENT => return,
            OT_CHART => {
                self.charts += 1;
                return;
            }
            _ => {}
        }
        if let Some(id) = obj.storage {
            let path = format!("/MBD{id:08X}");
            if self.cx.cfb.is_storage(&path) {
                self.used_storages.insert(path.clone());
                if let Some(part) = self
                    .cx
                    .storage_object(&path, obj.prog_id.as_deref(), &location)
                {
                    out.push(Block::Embed(EmbedRef { part }));
                    return;
                }
            }
        }
        if let Some(index) = shape.props.blip {
            let max = self.cx.options.max_part_size;
            if let Some(blip) = self.store.get(index, None, max) {
                let part = self.cx.picture(blip, format!("blip:{index}"), &location);
                out.push(Block::Image(ImageRef {
                    part,
                    alt: shape.props.description.clone(),
                }));
            }
        }
        let text = obj
            .text
            .as_deref()
            .or(shape.props.word_art.as_deref())
            .unwrap_or_default();
        let mut blocks: Vec<Block> = text
            .split('\n')
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(Block::text)
            .collect();
        if let Some(link) = shape.props.hyperlink.as_deref().and_then(hyperlink::parse) {
            link_shape(&mut blocks, link);
        }
        push_text_box(out, blocks);
    }
}

/// An `IMDATA` picture: a Windows metafile, an OS/2 bitmap, or a file in
/// its native format.
fn picture_data(format: u16, data: &[u8]) -> Option<Blip> {
    let (data, content_type) = match format {
        0x0002 => {
            // A METAFILEPICT header (mapping mode, extent) precedes the
            // metafile; a bare header and end record draw nothing.
            let wmf = data
                .get(8..)
                .filter(|d| d.starts_with(&[1, 0, 9, 0]))
                .unwrap_or(data);
            if wmf.len() <= 24 {
                return None;
            }
            (wmf.to_vec(), "image/wmf")
        }
        0x0009 => (officeart::bmp_file(data)?, "image/bmp"),
        _ => (data.to_vec(), doctopus_core::sniff::sniff(data)?.mime),
    };
    Some(Blip {
        data,
        content_type,
        name: None,
    })
}

/// The top-left cell of a sheet anchor (`OfficeArtClientAnchorSheet`).
fn anchor_cell(anchor: &[u8]) -> Option<CellRef> {
    let mut r = Cursor::new(anchor);
    r.skip(2)?;
    let col = r.u16()?;
    r.skip(2)?;
    let row = r.u16()?;
    Some(CellRef::new(u32::from(row), u32::from(col)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(kind: u16, data: &[u8]) -> Vec<u8> {
        let mut out = kind.to_le_bytes().to_vec();
        out.extend_from_slice(&(data.len() as u16).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn globals_list_sheets_and_encryption() {
        let mut stream = rec(rt::BOF, &[0x00, 0x06, 0x05, 0x00]);
        let mut sheet = 1234u32.to_le_bytes().to_vec();
        sheet.extend_from_slice(&[0, 0, 4, 0]);
        sheet.extend_from_slice(b"Data");
        stream.extend(rec(rt::BOUNDSHEET, &sheet));
        stream.extend(rec(rt::FILEPASS, &[1, 0]));
        stream.extend(rec(rt::EOF, &[]));
        let g = Globals::read(&stream);
        assert!(g.biff8 && g.encrypted);
        assert_eq!(g.sheets, vec![("Data".to_string(), 1234)]);
    }

    #[test]
    fn notes_take_their_text_from_the_object_and_text_records() {
        let mut stream = rec(rt::BOF, &[0x00, 0x06, 0x10, 0x00]);
        let mut cmo = vec![0x15, 0x00, 0x12, 0x00, 0x19, 0x00, 0x07, 0x00];
        cmo.extend_from_slice(&[0; 14]);
        cmo.extend_from_slice(&[0, 0, 0, 0]);
        stream.extend(rec(rt::OBJ, &cmo));
        let mut txo = vec![0; 18];
        txo[10] = 9;
        stream.extend(rec(rt::TXO, &txo));
        let mut cont = vec![0];
        cont.extend_from_slice(b"Ana:\ncheck");
        stream.extend(rec(rt::CONTINUE, &cont[..10]));
        let mut note = vec![2, 0, 1, 0, 0, 0, 7, 0, 3, 0, 0];
        note.extend_from_slice(b"Ana");
        stream.extend(rec(rt::NOTE, &note));
        stream.extend(rec(rt::EOF, &[]));
        let x = SheetExtras::read(&stream, 0, true);
        assert_eq!(
            x.notes().get(&(2, 1)).map(String::as_str),
            Some("Ana:\nchec")
        );
    }

    #[test]
    fn embedded_objects_name_their_storage() {
        // ftCmo (picture), then ftPictFmla with a 6-byte formula area.
        let mut data = vec![0x15, 0x00, 0x12, 0x00, 0x08, 0x00, 0x02, 0x00];
        data.extend_from_slice(&[0; 14]);
        let mut fmla = vec![0x06, 0x00, 0x00, 0x00, 0, 0, 0, 0];
        fmla.extend_from_slice(&0x0000_ABCDu32.to_le_bytes());
        data.extend_from_slice(&0x0009u16.to_le_bytes());
        data.extend_from_slice(&(fmla.len() as u16).to_le_bytes());
        data.extend(fmla);
        let o = obj(&data);
        assert_eq!((o.kind, o.id, o.storage), (8, 2, Some(0xABCD)));
    }
}
