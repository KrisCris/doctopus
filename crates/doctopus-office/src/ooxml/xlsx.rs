//! Excel workbooks (SpreadsheetML), and binary workbooks (`.xlsb`) for
//! their cells.
//!
//! Cell values, formulas, merged ranges and hyperlinks come from
//! [calamine](https://docs.rs/calamine); notes, threaded comments, drawings
//! (pictures, charts, text boxes), embedded objects and tables are read from
//! the package directly.

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek};

use calamine::{Reader as _, SheetType, SheetVisible, Xlsb, XlsbError, Xlsx, XlsxError};
use doctopus_core::container::{Zip, replace_in_zip};
use doctopus_core::{
    Block, CellRef, Document, EmbedRef, Error, Group, GroupKind, Input, Result, Section,
    SectionKind,
};

use super::context::Context;
use super::drawing;
use super::package::Relationships;
use super::xml::{self, Element};
use crate::binary::u32_at;
use crate::sheet::{self, MAX_CELLS, SheetCells, header_footer_text};

/// Links spanning more cells than this are attached to the first cells only.
const MAX_LINK_CELLS: u64 = 1_000;
/// `BrtBundleSh`: a sheet of a binary workbook ([MS-XLSB] 2.4.304).
const BRT_BUNDLE_SH: usize = 0x009C;

/// Read an `.xlsx` workbook.
pub(crate) fn read(input: &Input<'_>) -> Result<Document> {
    let mut cx = Context::open(input)?;
    let (main, workbook) = cx.main_xml()?;
    let rels = cx.rels(&main);
    let parts = sheet_parts(&workbook, &rels);
    let persons = persons(&mut cx, &rels);
    let mut book = calamine::open_workbook_from_rs::<Xlsx<_>, _>(Cursor::new(input.data))
        .map_err(|e| Error::malformed(input.format.name, e.to_string()))?;
    let sheets = book.sheets_metadata().to_vec();
    let mut number = 0;
    for meta in sheets {
        if meta.typ == SheetType::Vba {
            continue;
        }
        number += 1;
        let mut section = Section::new(SectionKind::Sheet)
            .with_number(number)
            .with_name(meta.name.clone());
        mark_visibility(&mut section, meta.visible);
        let part = parts.get(&meta.name).cloned();
        let location = format!("sheet \"{}\"", meta.name);
        if meta.typ == SheetType::ChartSheet {
            section
                .properties
                .insert("type".into(), "chart sheet".into());
            if let Some(part) = &part {
                let part_rels = cx.rels(part);
                drawings(
                    &mut cx,
                    &part_rels,
                    &location,
                    &meta.name,
                    &mut section.blocks,
                );
            }
            cx.doc.sections.push(section);
            continue;
        }
        let mut cells = SheetCells::default();
        if let Err(e) = xlsx_cells(&mut book, &meta.name, cx.options.formulas, &mut cells) {
            cx.doc.warn(format!(
                "cells of sheet \"{}\" could not be read: {e}",
                meta.name
            ));
        }
        match book.merge_cells_by_sheet_name(&meta.name) {
            Ok(merged) => cells.merged = merged,
            Err(e) => cx.doc.warn(format!(
                "merged cells of sheet \"{}\" could not be read: {e}",
                meta.name
            )),
        }
        if let Ok(links) = book.hyperlinks_by_sheet_name(&meta.name) {
            for link in links {
                let target = match (&link.target, &link.location) {
                    (Some(t), _) => t.clone(),
                    (None, Some(l)) => format!("#{l}"),
                    _ => continue,
                };
                let d = link.range;
                let mut n = 0;
                'cells: for row in d.start.0..=d.end.0 {
                    for col in d.start.1..=d.end.1 {
                        n += 1;
                        if n > MAX_LINK_CELLS {
                            break 'cells;
                        }
                        cells
                            .links
                            .entry((row, col))
                            .or_insert_with(|| target.clone());
                    }
                }
            }
        }
        let part_rels = part.as_deref().map(|p| cx.rels(p)).unwrap_or_default();
        cells.comments = comments(&mut cx, &part_rels, &persons);
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
        let tables = table_names(&mut cx, &part_rels);
        if !tables.is_empty() {
            section
                .properties
                .insert("tables".into(), tables.join(", "));
        }
        drawings(
            &mut cx,
            &part_rels,
            &location,
            &meta.name,
            &mut section.blocks,
        );
        if let Some(part) = &part {
            let wanted: &[&str] = if has_objects(&part_rels) {
                &["x:headerFooter", "x:oleObject"]
            } else {
                &["x:headerFooter"]
            };
            let extras = match cx.pkg.read(part, &mut cx.doc) {
                Some(data) => xml::parse_filtered(&data, wanted),
                None => Vec::new(),
            };
            objects(
                &mut cx,
                &extras,
                &part_rels,
                &meta.name,
                &mut section.blocks,
            );
            if let Some(hf) = extras.iter().find(|e| e.is("x:headerFooter")) {
                section.blocks.extend(headers_footers(hf));
            }
        }
        cx.doc.sections.push(section);
    }
    Ok(cx.doc)
}

/// Read an `.xlsb` workbook: cells, formulas and document properties.
pub(crate) fn read_binary(input: &Input<'_>) -> Result<Document> {
    let mut cx = Context::open(input)?;
    let repaired = standard_sheet_records(input);
    let data = repaired.as_deref().unwrap_or(input.data);
    let mut book = calamine::open_workbook_from_rs::<Xlsb<_>, _>(Cursor::new(data))
        .map_err(|e| Error::malformed(input.format.name, e.to_string()))?;
    let sheets = book.sheets_metadata().to_vec();
    let mut number = 0;
    for meta in sheets {
        if meta.typ == SheetType::Vba {
            continue;
        }
        number += 1;
        let mut section = Section::new(SectionKind::Sheet)
            .with_number(number)
            .with_name(meta.name.clone());
        mark_visibility(&mut section, meta.visible);
        let mut cells = SheetCells::default();
        if let Err(e) = xlsb_cells(&mut book, &meta.name, cx.options.formulas, &mut cells) {
            cx.doc.warn(format!(
                "cells of sheet \"{}\" could not be read: {e}",
                meta.name
            ));
        }
        let built = sheet::build(cells);
        if built.truncated {
            cx.doc.warn(format!(
                "sheet \"{}\" was cut off after {MAX_CELLS} cells",
                meta.name
            ));
        }
        if !built.grid.rows.is_empty() {
            section.blocks.push(Block::Grid(built.grid));
        }
        cx.doc.sections.push(section);
    }
    Ok(cx.doc)
}

/// Workbooks written by beta versions of Excel 2007 store their sheets
/// (`BrtBundleSh`) with 8 bytes before the tab id instead of 4, and
/// calamine fails on them. Apache POI reads them as "beta" files. Returns
/// the package with those records rewritten in the standard layout, or
/// `None` when it has none.
fn standard_sheet_records(input: &Input<'_>) -> Option<Vec<u8>> {
    let mut zip = Zip::new(input.data, input.options.max_part_size).ok()?;
    let name = zip.find("xl/workbook.bin")?.to_string();
    let workbook = zip.read(&name).ok()??;
    let fixed = rewrite_beta_sheets(&workbook)?;
    replace_in_zip(input.data, &[(&name, &fixed)]).ok()
}

/// The records of `workbook.bin` with beta sheet records rewritten, or
/// `None` when there are none or the records do not parse.
fn rewrite_beta_sheets(workbook: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(workbook.len());
    let mut changed = false;
    let mut at = 0;
    while at < workbook.len() {
        let start = at;
        let kind = record_number(workbook, &mut at, 2)?;
        let header = at;
        let size = record_number(workbook, &mut at, 4)?;
        let body = workbook.get(at..at.checked_add(size)?)?;
        at += size;
        if kind == BRT_BUNDLE_SH && is_beta_sheet(body) {
            // Keep the sheet state when it is a valid one; drop the 4
            // extra bytes.
            let state = u32_at(body, 0).filter(|s| *s <= 2).unwrap_or(0);
            out.extend_from_slice(&workbook[start..header]);
            push_record_number(&mut out, size - 4);
            out.extend_from_slice(&state.to_le_bytes());
            out.extend_from_slice(&body[8..]);
            changed = true;
        } else {
            out.extend_from_slice(&workbook[start..at]);
        }
    }
    changed.then_some(out)
}

/// A standard record has the tab id (1 to 65535) at offset 4; a beta one
/// has 0 there and the tab id at offset 8, followed by the relationship id.
fn is_beta_sheet(body: &[u8]) -> bool {
    let relationship = u32_at(body, 12).filter(|n| *n != u32::MAX);
    u32_at(body, 4) == Some(0)
        && u32_at(body, 8).is_some_and(|tab| (1..=65_535).contains(&tab))
        && relationship.is_some_and(|n| 16 + 2 * n as usize <= body.len())
}

/// A record type or size: 7 bits per byte, low bits first, the high bit
/// set on every byte but the last.
fn record_number(data: &[u8], at: &mut usize, max_bytes: usize) -> Option<usize> {
    let mut value = 0;
    for i in 0..max_bytes {
        let byte = *data.get(*at)?;
        *at += 1;
        value |= usize::from(byte & 0x7F) << (7 * i);
        if byte & 0x80 == 0 {
            break;
        }
    }
    Some(value)
}

fn push_record_number(out: &mut Vec<u8>, mut value: usize) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Read the cells of a sheet one by one: calamine's whole-sheet ranges are
/// dense, so a sheet with two cells far apart would take gigabytes.
fn xlsx_cells<RS: Read + Seek>(
    book: &mut Xlsx<RS>,
    name: &str,
    formulas: bool,
    cells: &mut SheetCells,
) -> std::result::Result<(), XlsxError> {
    let mut reader = match book.worksheet_cells_reader(name) {
        Ok(reader) => reader,
        // Dialog and macro sheets have no cells to read.
        Err(XlsxError::NotAWorksheet(_)) => return Ok(()),
        Err(e) => return Err(e),
    };
    if formulas {
        while let Some(cell) = reader.next_cell_with_formula()? {
            let room = cells.add_value(cell.pos, cell.value.into());
            let formula_room = cell
                .formula
                .as_deref()
                .is_none_or(|f| cells.add_formula(cell.pos, f));
            if !room || !formula_room {
                break;
            }
        }
    } else {
        while let Some(cell) = reader.next_cell()? {
            if !cells.add_value(cell.get_position(), cell.get_value().clone().into()) {
                break;
            }
        }
    }
    Ok(())
}

/// [`xlsx_cells`] for a binary workbook.
fn xlsb_cells<RS: Read + Seek>(
    book: &mut Xlsb<RS>,
    name: &str,
    formulas: bool,
    cells: &mut SheetCells,
) -> std::result::Result<(), XlsbError> {
    let mut reader = book.worksheet_cells_reader(name)?;
    while let Some(cell) = reader.next_cell()? {
        if !cells.add_value(cell.get_position(), cell.get_value().clone().into()) {
            break;
        }
    }
    drop(reader);
    if formulas {
        let mut reader = book.worksheet_cells_reader(name)?;
        while let Some(cell) = reader.next_formula()? {
            if !cells.add_formula(cell.get_position(), cell.get_value()) {
                break;
            }
        }
    }
    Ok(())
}

fn mark_visibility(section: &mut Section, visible: SheetVisible) {
    match visible {
        SheetVisible::Visible => {}
        SheetVisible::Hidden => section.hidden = true,
        SheetVisible::VeryHidden => {
            section.hidden = true;
            section
                .properties
                .insert("visibility".into(), "very hidden".into());
        }
    }
}

/// Sheet name → sheet part, from `xl/workbook.xml`.
fn sheet_parts(workbook: &Element, rels: &Relationships) -> HashMap<String, String> {
    let Some(sheets) = workbook.child("x:sheets") else {
        return HashMap::new();
    };
    sheets
        .children_named("x:sheet")
        .filter_map(|s| {
            let name = s.attr("name")?.to_string();
            let target = rels.target(s.attr("r:id")?)?.to_string();
            Some((name, target))
        })
        .collect()
}

/// Authors of threaded comments: person id → display name.
fn persons(cx: &mut Context<'_>, rels: &Relationships) -> HashMap<String, String> {
    let Some(part) = rels.first_target("person").map(str::to_string) else {
        return HashMap::new();
    };
    let Some(root) = cx.xml(&part) else {
        return HashMap::new();
    };
    root.children_named("tc:person")
        .filter_map(|p| {
            Some((
                p.attr("id")?.to_string(),
                p.attr("displayName")?.to_string(),
            ))
        })
        .collect()
}

/// Notes and threaded comments of a sheet, by cell. Threaded comments
/// replace the placeholder notes Excel writes for them.
fn comments(
    cx: &mut Context<'_>,
    rels: &Relationships,
    persons: &HashMap<String, String>,
) -> HashMap<(u32, u32), String> {
    let mut out: HashMap<(u32, u32), String> = HashMap::new();
    if let Some(part) = rels.first_target("comments").map(str::to_string)
        && let Some(root) = cx.xml(&part)
    {
        let authors: Vec<String> = root
            .child("x:authors")
            .map(|a| a.children_named("x:author").map(Element::text).collect())
            .unwrap_or_default();
        for comment in root.find_all("x:comment") {
            let Some(pos) = comment.attr("ref").and_then(cell_position) else {
                continue;
            };
            let text = comment
                .child("x:text")
                .map(Element::text)
                .unwrap_or_default();
            let text = text.trim();
            let author = comment
                .attr_parse::<usize>("authorId")
                .and_then(|i| authors.get(i))
                .map(|a| a.trim())
                .filter(|a| !a.is_empty() && !text.starts_with(*a));
            let entry = match author {
                Some(author) => format!("{author}: {text}"),
                None => text.to_string(),
            };
            if !entry.is_empty() {
                out.insert(pos, entry);
            }
        }
    }
    for part in rels
        .of_kind("threadedComment")
        .filter(|r| !r.external)
        .map(|r| r.target.clone())
        .collect::<Vec<_>>()
    {
        let Some(root) = cx.xml(&part) else {
            continue;
        };
        let mut threads: HashMap<(u32, u32), Vec<String>> = HashMap::new();
        for comment in root.children_named("tc:threadedComment") {
            let Some(pos) = comment.attr("ref").and_then(cell_position) else {
                continue;
            };
            let who = comment
                .attr("personId")
                .and_then(|id| persons.get(id))
                .map_or("someone", String::as_str);
            let text = comment
                .child("tc:text")
                .map(Element::text)
                .unwrap_or_default();
            threads
                .entry(pos)
                .or_default()
                .push(format!("{who}: {}", text.trim()));
        }
        for (pos, entries) in threads {
            out.insert(pos, entries.join(" | "));
        }
    }
    out
}

/// Names and ranges of the tables (list objects) on a sheet.
fn table_names(cx: &mut Context<'_>, rels: &Relationships) -> Vec<String> {
    let targets: Vec<String> = rels
        .of_kind("table")
        .filter(|r| !r.external)
        .map(|r| r.target.clone())
        .collect();
    let mut out = Vec::new();
    for target in targets {
        if let Some(root) = cx.xml(&target) {
            let name = root.attr("displayName").or_else(|| root.attr("name"));
            match (name, root.attr("ref")) {
                (Some(name), Some(range)) => out.push(format!("{name} ({range})")),
                (Some(name), None) => out.push(name.to_string()),
                _ => {}
            }
        }
    }
    out
}

/// Pictures, charts and shapes drawn on a sheet, in drawing order.
fn drawings(
    cx: &mut Context<'_>,
    sheet_rels: &Relationships,
    location: &str,
    sheet: &str,
    out: &mut Vec<Block>,
) {
    let targets: Vec<String> = sheet_rels
        .of_kind("drawing")
        .filter(|r| !r.external)
        .map(|r| r.target.clone())
        .collect();
    for target in targets {
        let Some(root) = cx.xml(&target) else {
            continue;
        };
        let rels = cx.rels(&target);
        for anchor in root.children() {
            let at = anchor
                .child("xdr:from")
                .and_then(|from| {
                    let col = from.child("xdr:col")?.text().trim().parse().ok()?;
                    let row = from.child("xdr:row")?.text().trim().parse().ok()?;
                    Some(format!("sheet \"{sheet}\" cell {}", CellRef::new(row, col)))
                })
                .unwrap_or_else(|| location.to_string());
            for shape in anchor.children() {
                drawing::shape(cx, &rels, shape, &at, out);
            }
        }
    }
}

fn has_objects(rels: &Relationships) -> bool {
    rels.iter()
        .any(|r| matches!(r.kind(), "oleObject" | "package"))
}

/// Embedded OLE objects and packages of a sheet, from its `x:oleObject`
/// elements and, for objects not listed there, its relationships.
fn objects(
    cx: &mut Context<'_>,
    elements: &[Element],
    rels: &Relationships,
    sheet: &str,
    out: &mut Vec<Block>,
) {
    if !has_objects(rels) {
        return;
    }
    let mut seen: Vec<&str> = Vec::new();
    for ole in elements.iter().filter(|e| e.is("x:oleObject")) {
        let Some(id) = ole.attr("r:id") else {
            continue;
        };
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        let at = ole
            .find("x:from")
            .and_then(|from| {
                let col = from.child("xdr:col")?.text().trim().parse().ok()?;
                let row = from.child("xdr:row")?.text().trim().parse().ok()?;
                Some(format!("sheet \"{sheet}\" cell {}", CellRef::new(row, col)))
            })
            .unwrap_or_else(|| format!("sheet \"{sheet}\""));
        if let Some(part) = cx.object(rels, id, ole.attr("progId"), &at) {
            out.push(Block::Embed(EmbedRef { part }));
        }
    }
    // Objects related to the sheet but not listed (damaged or unusual files).
    let rest: Vec<String> = rels
        .iter()
        .filter(|r| matches!(r.kind(), "oleObject" | "package") && !seen.contains(&r.id.as_str()))
        .map(|r| r.id.clone())
        .collect();
    for id in rest {
        if let Some(part) = cx.object(rels, &id, None, &format!("sheet \"{sheet}\"")) {
            out.push(Block::Embed(EmbedRef { part }));
        }
    }
}

/// The printed page headers and footers of a sheet.
fn headers_footers(hf: &Element) -> Vec<Block> {
    let first = hf.attr_bool("differentFirst").unwrap_or(false);
    let even = hf.attr_bool("differentOddEven").unwrap_or(false);
    let entries = [
        ("x:oddHeader", GroupKind::Header, None, true),
        ("x:oddFooter", GroupKind::Footer, None, true),
        (
            "x:firstHeader",
            GroupKind::Header,
            Some("first page"),
            first,
        ),
        (
            "x:firstFooter",
            GroupKind::Footer,
            Some("first page"),
            first,
        ),
        ("x:evenHeader", GroupKind::Header, Some("even pages"), even),
        ("x:evenFooter", GroupKind::Footer, Some("even pages"), even),
    ];
    entries
        .into_iter()
        .filter(|(_, _, _, used)| *used)
        .filter_map(|(name, kind, title, _)| {
            let text = header_footer_text(&hf.child(name)?.text());
            (!text.is_empty()).then(|| {
                Block::Group(Group {
                    kind,
                    title: title.map(str::to_string),
                    blocks: vec![Block::text(text)],
                })
            })
        })
        .collect()
}

/// `B7` → (row 6, column 1).
fn cell_position(reference: &str) -> Option<(u32, u32)> {
    let first = reference.split(':').next()?;
    let cell: CellRef = first.parse().ok()?;
    Some((cell.row, cell.col))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `BrtBundleSh` record for sheet 1, `rId1`, "Data", with `prefix`
    /// before the tab id.
    fn bundle(prefix: &[u8]) -> Vec<u8> {
        let mut body = prefix.to_vec();
        body.extend_from_slice(&1u32.to_le_bytes());
        for text in ["rId1", "Data"] {
            body.extend_from_slice(&(text.len() as u32).to_le_bytes());
            body.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        }
        let mut record = vec![0x9C, 0x01];
        push_record_number(&mut record, body.len());
        record.extend(body);
        record
    }

    #[test]
    fn beta_sheet_records_are_rewritten() {
        let begin = [0x83, 0x01, 0x00];
        let mut workbook = begin.to_vec();
        workbook.extend(bundle(&[0; 8]));
        let fixed = rewrite_beta_sheets(&workbook).unwrap();
        let mut expected = begin.to_vec();
        expected.extend(bundle(&[0; 4]));
        assert_eq!(fixed, expected);
        assert_eq!(rewrite_beta_sheets(&expected), None);
    }

    #[test]
    fn record_numbers_use_seven_bits_per_byte() {
        let mut out = Vec::new();
        push_record_number(&mut out, 0x9C);
        assert_eq!(out, [0x9C, 0x01]);
        let mut at = 0;
        assert_eq!(record_number(&out, &mut at, 2), Some(0x9C));
        assert_eq!(at, 2);
    }

    #[test]
    fn parses_cell_positions() {
        assert_eq!(cell_position("B7"), Some((6, 1)));
        assert_eq!(cell_position("AA1:AB3"), Some((0, 26)));
        assert_eq!(cell_position("7B"), None);
    }
}
