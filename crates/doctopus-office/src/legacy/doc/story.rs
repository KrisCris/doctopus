//! Reading the stories of a document into blocks: paragraphs and their
//! runs, fields, tables, pictures, embedded objects, floating drawings and
//! note references.

use std::collections::HashSet;
use std::ops::Range;

use doctopus_core::{
    Block, EmbedRef, Group, GroupKind, ImageRef, Inline, Link, NoteKind, NoteRef, Style, Table,
    TableCell, TableRow, push_text,
};

use super::props::{CharProps, ParaProps, RowDef, VMerge};
use super::{Doc, Story, note_label};
use crate::binary::{Cursor, i16_at};
use crate::legacy::context::Context;
use crate::legacy::officeart::{self, Records, Shape, rt};
use crate::numbering::{Counters, ListLabel, NumRef, Numbering};
use crate::word::{hyperlink_target, is_blank, paragraph_block, symbol_char, trim_inlines};

/// Characters with a meaning of their own.
mod ch {
    pub(super) const PICTURE: u16 = 0x01;
    pub(super) const CELL: u16 = 0x07;
    pub(super) const DRAWING: u16 = 0x08;
    pub(super) const TAB: u16 = 0x09;
    pub(super) const LINE_BREAK: u16 = 0x0B;
    pub(super) const PAGE_BREAK: u16 = 0x0C;
    pub(super) const PARAGRAPH: u16 = 0x0D;
    pub(super) const FIELD_BEGIN: u16 = 0x13;
    pub(super) const FIELD_SEPARATOR: u16 = 0x14;
    pub(super) const FIELD_END: u16 = 0x15;
    pub(super) const NON_BREAKING_HYPHEN: u16 = 0x1E;
    pub(super) const SYMBOL: u16 = 0x28;
}

/// `PICF.mfpf.mm` of a picture whose header is followed by a name.
const MM_SHAPEFILE: i16 = 0x66;
/// List instance indexes above this are Word 6 compatibility values that
/// do not number the paragraph.
const MAX_LIST_INSTANCE: u16 = 0x07FE;
/// How far apart (in twips) cell edges may be and still line up.
const EDGE_TOLERANCE: i32 = 20;
/// How deeply text boxes are followed (a damaged file could loop).
const MAX_NESTING: usize = 4;

/// A field (`0x13` begin ... `0x14` separator ... `0x15` end) being read.
#[derive(Debug, Default)]
struct Field {
    instruction: String,
    /// Past the separator: the text now is the field's displayed result.
    in_result: bool,
    /// For `HYPERLINK` fields: the target and the result collected so far.
    link: Option<String>,
    result: Vec<Inline>,
    /// For `EMBED` fields: the object was found, so the picture in the
    /// result, a preview of the object, is not shown.
    object: bool,
}

/// A paragraph of a story: its content and the character that ends it.
#[derive(Debug, Clone)]
struct Para {
    range: Range<u32>,
    mark: u16,
    props: ParaProps,
}

/// Reads the stories of one document into blocks.
pub(super) struct Walker<'w, 'a, 'd> {
    cx: &'w mut Context<'a>,
    doc: &'w Doc<'d>,
    numbering: &'w Numbering,
    counters: Counters,
    /// Where parts found now are said to be.
    pub(super) area: String,
    /// Text of the most recent heading, to locate parts.
    heading: Option<String>,
    story: Story,
    fields: Vec<Field>,
    /// Blocks met inside the current paragraph (text boxes, objects) that
    /// are emitted after it.
    floats: Vec<Block>,
    pub(super) insertions: usize,
    pub(super) deletions: usize,
    /// Whether the previous run was inserted or deleted, so that a change
    /// is counted once however many runs it spans.
    revision: (bool, bool),
    /// `ObjectPool` storages shown in the text.
    pub(super) objects: HashSet<String>,
    /// Text boxes shown, each once however many anchors refer to it.
    text_boxes: HashSet<(u32, u32)>,
    nesting: usize,
}

impl<'w, 'a, 'd> Walker<'w, 'a, 'd> {
    pub(super) fn new(cx: &'w mut Context<'a>, doc: &'w Doc<'d>, numbering: &'w Numbering) -> Self {
        Self {
            cx,
            doc,
            numbering,
            counters: Counters::default(),
            area: "body".into(),
            heading: None,
            story: Story::Main,
            fields: Vec::new(),
            floats: Vec::new(),
            insertions: 0,
            deletions: 0,
            revision: (false, false),
            objects: HashSet::new(),
            text_boxes: HashSet::new(),
            nesting: 0,
        }
    }

    fn location(&self) -> String {
        match &self.heading {
            Some(h) if self.area == "body" => format!("under \"{h}\""),
            _ => self.area.clone(),
        }
    }

    /// Read a range of a story into blocks.
    pub(super) fn story(&mut self, story: Story, range: Range<u32>) -> Vec<Block> {
        let saved = std::mem::replace(&mut self.story, story);
        let paras = self.paragraphs(range);
        let mut out = Vec::new();
        let mut i = 0;
        while i < paras.len() {
            if paras[i].props.depth() > 0 {
                let (table, next) = self.table(&paras, i, 1);
                out.push(Block::Table(table));
                i = next;
            } else {
                self.paragraph(&paras[i], &mut out);
                i += 1;
            }
        }
        self.fields.clear();
        self.story = saved;
        out
    }

    /// Split a range into paragraphs at paragraph marks, cell marks, and
    /// section breaks.
    fn paragraphs(&self, range: Range<u32>) -> Vec<Para> {
        let doc = self.doc;
        let mut out = Vec::new();
        let mut start = range.start;
        for cp in range.clone() {
            let unit = doc.text.unit(cp);
            let ends = match unit {
                ch::PARAGRAPH | ch::CELL => true,
                ch::PAGE_BREAK => doc.ends_paragraph(cp),
                _ => false,
            };
            if ends {
                out.push(Para {
                    range: start..cp,
                    mark: unit,
                    props: doc.para_props(cp),
                });
                start = cp + 1;
            }
        }
        if start < range.end {
            out.push(Para {
                range: start..range.end,
                mark: 0,
                props: doc.para_props(range.end - 1),
            });
        }
        out
    }

    fn paragraph(&mut self, para: &Para, out: &mut Vec<Block>) {
        let doc = self.doc;
        let props = &para.props;
        let style = doc.styles.paras(props.istd);
        let heading = match props.own.outline {
            Some(outline) => (outline < 9).then_some(outline + 1),
            None => doc.styles.heading_level(props.istd),
        };
        let list = props
            .own
            .ilfo
            .or(style.ilfo)
            .filter(|&i| i > 0 && i <= MAX_LIST_INSTANCE);
        let level = props.own.ilvl.or(style.ilvl).unwrap_or(0).min(8);

        self.revision = (false, false);
        let mut content = Vec::new();
        self.inlines(para.range.clone(), props.istd, &mut content);
        self.close_paragraph_fields(&mut content);
        trim_inlines(&mut content);
        let floats = std::mem::take(&mut self.floats);

        let label = match list {
            Some(ilfo) => {
                let num = NumRef {
                    num_id: ilfo.to_string(),
                    level: Some(level),
                };
                self.counters.next(self.numbering, &num)
            }
            None => ListLabel::None,
        };
        if !is_blank(&content) {
            let (block, heading) = paragraph_block(content, heading, label, level);
            if heading.is_some() {
                self.heading = heading;
            }
            out.push(block);
        }
        out.extend(floats);
    }

    // ----- tables ------------------------------------------------------------

    /// Read the table starting at paragraph `i` at nesting depth `depth`;
    /// returns the table and the index of the first paragraph after it.
    fn table(&mut self, paras: &[Para], mut i: usize, depth: usize) -> (Table, usize) {
        let mut rows: Vec<(Vec<Vec<Block>>, Option<RowDef>)> = Vec::new();
        let mut cells: Vec<Vec<Block>> = Vec::new();
        let mut cell: Vec<Block> = Vec::new();
        while let Some(para) = paras.get(i) {
            let d = para.props.depth();
            if d < depth {
                break;
            }
            if d > depth {
                let (inner, next) = self.table(paras, i, depth + 1);
                cell.push(Block::Table(inner));
                i = next;
                continue;
            }
            // Rows end with a paragraph of their own that holds the row's
            // properties; nested tables mark cells and rows with flags.
            let (row_end, cell_end) = if depth == 1 {
                (para.props.ttp, para.mark == ch::CELL)
            } else {
                (para.props.inner_ttp, para.props.inner_cell)
            };
            if row_end {
                if !cell.is_empty() {
                    cells.push(std::mem::take(&mut cell));
                }
                rows.push((std::mem::take(&mut cells), para.props.row.clone()));
                i += 1;
                continue;
            }
            self.paragraph(para, &mut cell);
            if cell_end {
                cells.push(std::mem::take(&mut cell));
            }
            i += 1;
        }
        if !cell.is_empty() {
            cells.push(cell);
        }
        if !cells.is_empty() {
            rows.push((cells, None));
        }
        (build_table(rows), i)
    }

    // ----- runs --------------------------------------------------------------

    fn inlines(&mut self, range: Range<u32>, istd: u16, content: &mut Vec<Inline>) {
        let doc = self.doc;
        let para_style = doc.styles.chars(istd).unwrap_or_default();
        for (run, bin) in doc.text.char_runs(range, &doc.chpx) {
            let grpprl = bin.map_or(&[][..], |b| b.grpprl);
            let props = CharProps::resolve(
                para_style,
                |i| doc.styles.chars(i),
                [grpprl, doc.text.piece_sprms(run.start)].into_iter(),
            );
            self.run(run, &props, content);
        }
    }

    fn run(&mut self, range: Range<u32>, props: &CharProps, content: &mut Vec<Inline>) {
        let doc = self.doc;
        let style = Style {
            bold: props.bold,
            italic: props.italic,
            strike: props.strike,
            superscript: props.superscript,
            subscript: props.subscript,
        };
        let visible = !props.hidden && !props.deleted;
        if !range.is_empty() {
            if props.inserted && visible && !self.revision.0 {
                self.insertions += 1;
            }
            if props.deleted && !self.revision.1 {
                self.deletions += 1;
            }
            self.revision = (props.inserted, props.deleted);
        }
        let mut text: Vec<u16> = Vec::new();
        for cp in range {
            let unit = doc.text.unit(cp);
            match unit {
                ch::FIELD_BEGIN => {
                    self.flush(&mut text, style, content);
                    self.fields.push(Field::default());
                    continue;
                }
                ch::FIELD_SEPARATOR => {
                    self.flush(&mut text, style, content);
                    if let Some(field) = self.fields.last_mut() {
                        field.in_result = true;
                        field.link = hyperlink_target(&field.instruction);
                    }
                    // An embedded object's separator names its storage.
                    if props.ole
                        && visible
                        && let Some(id) = props.pic
                    {
                        let shown = self.object(id);
                        if let Some(field) = self.fields.last_mut() {
                            field.object = shown;
                        }
                    }
                    continue;
                }
                ch::FIELD_END => {
                    self.flush(&mut text, style, content);
                    self.end_field(content);
                    continue;
                }
                _ => {}
            }
            // Field instructions are not displayed. Only the innermost
            // field counts, as in the .docx reader.
            if let Some(field) = self.fields.last_mut().filter(|f| !f.in_result) {
                if unit >= 0x20 {
                    field
                        .instruction
                        .push(char::from_u32(u32::from(unit)).unwrap_or(' '));
                }
                continue;
            }
            if !visible {
                continue;
            }
            if let Some(note) = self.note_ref(cp) {
                self.flush(&mut text, style, content);
                self.sink(content).push(Inline::NoteRef(note));
                continue;
            }
            // Symbols inserted from a symbol font; usually but not always
            // marked special.
            if unit == ch::SYMBOL
                && let Some((font, code)) = props.symbol
            {
                if let Some(c) = symbol_char(self.font(Some(font)), u32::from(code)) {
                    push_char(&mut text, c);
                }
                continue;
            }
            if props.special {
                match unit {
                    ch::PICTURE if !self.in_object() => {
                        self.flush(&mut text, style, content);
                        self.picture_char(props, content);
                    }
                    ch::DRAWING if !self.in_object() => {
                        self.flush(&mut text, style, content);
                        let story = self.story;
                        let rel = cp - doc.stories.range(story).start;
                        if let Some(shape) = doc.drawings.anchored(story, rel) {
                            self.shape(shape, content);
                        }
                    }
                    // Note marks inside notes, separators, and other
                    // special characters show nothing.
                    u if u < 0x20 => {}
                    u => text.push(u),
                }
                continue;
            }
            match unit {
                ch::LINE_BREAK => {
                    self.flush(&mut text, style, content);
                    self.sink(content).push(Inline::LineBreak);
                }
                ch::TAB => text.push(u16::from(b' ')),
                ch::NON_BREAKING_HYPHEN => text.push(u16::from(b'-')),
                u if u < 0x20 => {}
                0xF000..=0xF0FF => {
                    if let Some(c) = symbol_char(self.font(props.font), u32::from(unit)) {
                        push_char(&mut text, c);
                    }
                }
                u => text.push(u),
            }
        }
        self.flush(&mut text, style, content);
    }

    /// A font's name, for symbol characters.
    fn font(&self, index: Option<u16>) -> &str {
        index
            .or(self.doc.styles.default_font)
            .and_then(|i| self.doc.fonts.get(usize::from(i)))
            .map_or("", String::as_str)
    }

    /// Add the collected text.
    fn flush(&mut self, text: &mut Vec<u16>, style: Style, content: &mut Vec<Inline>) {
        if text.is_empty() {
            return;
        }
        let s = String::from_utf16_lossy(text);
        text.clear();
        push_text(self.sink(content), &s, style);
    }

    /// Where run content goes: into a hyperlink field being collected, or
    /// straight into the paragraph.
    fn sink<'s>(&'s mut self, content: &'s mut Vec<Inline>) -> &'s mut Vec<Inline> {
        match self.fields.last_mut() {
            Some(field) if field.in_result && field.link.is_some() => &mut field.result,
            _ => content,
        }
    }

    fn end_field(&mut self, content: &mut Vec<Inline>) {
        let Some(field) = self.fields.pop() else {
            return;
        };
        match field.link {
            Some(url) if !is_blank(&field.result) => self.sink(content).push(Inline::Link(Link {
                url,
                content: field.result,
            })),
            _ => self.sink(content).extend(field.result),
        }
    }

    /// Fields may span paragraphs (a table of contents does); a hyperlink
    /// still open at the end of a paragraph gives up its link and keeps
    /// its text.
    fn close_paragraph_fields(&mut self, content: &mut Vec<Inline>) {
        // Instructions do not span paragraphs; a field still reading its
        // instruction here is damaged.
        self.fields.retain(|f| f.in_result);
        for field in &mut self.fields {
            if field.link.take().is_some() {
                content.append(&mut field.result);
            }
        }
    }

    /// The note a main-text character refers to, if it is a reference
    /// (an automatic number or a custom mark).
    fn note_ref(&self, cp: u32) -> Option<NoteRef> {
        if self.story != Story::Main {
            return None;
        }
        let doc = self.doc;
        [
            (&doc.footnotes, NoteKind::Footnote),
            (&doc.endnotes, NoteKind::Endnote),
            (&doc.comments, NoteKind::Comment),
        ]
        .into_iter()
        .find_map(|(notes, kind)| {
            let index = notes.refs.find(cp)?;
            Some(NoteRef {
                kind,
                label: note_label(kind, index),
            })
        })
    }

    // ----- pictures, objects and drawings -------------------------------------

    /// Inside the result of a field whose object is shown.
    fn in_object(&self) -> bool {
        self.fields.last().is_some_and(|f| f.object)
    }

    /// A `0x01` character: an embedded object or an inline picture.
    fn picture_char(&mut self, props: &CharProps, content: &mut Vec<Inline>) {
        let Some(location) = props.pic else {
            return;
        };
        if props.ole {
            self.object(location);
        } else if !props.data
            && let Some(image) = self.picture(location)
        {
            self.sink(content).push(Inline::Image(image));
        }
    }

    /// An embedded object, shown after the paragraph. Returns whether it
    /// was found.
    fn object(&mut self, id: u32) -> bool {
        let path = format!("/ObjectPool/_{id}");
        if !self.cx.cfb.is_storage(&path) {
            let location = self.location();
            self.cx.doc.warn(format!(
                "an embedded object {location} is missing from the file; only its picture is shown"
            ));
            return false;
        }
        let prog_id = self.fields.iter().rev().find_map(|f| {
            let mut words = f.instruction.split_whitespace();
            match words.next() {
                Some(w) if w.eq_ignore_ascii_case("EMBED") => words.next().map(str::to_string),
                _ => None,
            }
        });
        let location = self.location();
        self.objects.insert(path.clone());
        match self.cx.storage_object(&path, prog_id.as_deref(), &location) {
            Some(part) => {
                self.floats.push(Block::Embed(EmbedRef { part }));
                true
            }
            None => false,
        }
    }

    /// An inline picture: a picture header in the `Data` stream followed by
    /// a shape and its picture.
    fn picture(&mut self, offset: u32) -> Option<ImageRef> {
        let data = self.doc.data;
        let start = offset as usize;
        let mut r = Cursor::at(data, start);
        let size = r.u32()? as usize;
        let header = usize::from(r.u16()?);
        let block = data.get(start..start.checked_add(size)?)?;
        let mut pos = header;
        if i16_at(block, 6)? == MM_SHAPEFILE {
            pos += 1 + usize::from(*block.get(pos)?);
        }
        let mut records = Records::new(block.get(pos..)?);
        let container = records.next().filter(|r| r.kind == rt::SP_CONTAINER)?;
        let shape = officeart::shape(&container);
        let origin = format!("data:{offset}");
        let part = match self.cx.doc.part_by_origin(&origin) {
            Some(part) => part.id.clone(),
            None => {
                let max = self.cx.options.max_part_size;
                let pictures: Vec<_> = records
                    .filter_map(|r| officeart::embedded_blip(&r, max))
                    .collect();
                let index = shape.props.blip.unwrap_or(1).saturating_sub(1) as usize;
                let blip = pictures.get(index).or_else(|| pictures.first())?.clone();
                let location = self.location();
                self.cx.picture(blip, origin, &location)
            }
        };
        Some(ImageRef {
            part,
            alt: shape.props.description.clone(),
        })
    }

    /// A floating shape: pictures go inline, text boxes after the
    /// paragraph.
    fn shape(&mut self, shape: &Shape<'d>, content: &mut Vec<Inline>) {
        if shape.is_deleted() {
            return;
        }
        if !shape.children.is_empty() {
            for child in &shape.children {
                self.shape(child, content);
            }
            return;
        }
        if let Some(index) = shape.props.blip
            && let Some(part) = self.store_picture(index)
        {
            self.sink(content).push(Inline::Image(ImageRef {
                part,
                alt: shape.props.description.clone(),
            }));
        }
        if let Some((story, range)) = self.text_box_range(shape) {
            self.text_box(story, range);
        } else if let Some(text) = &shape.props.word_art {
            let blocks: Vec<Block> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(Block::text)
                .collect();
            if !blocks.is_empty() {
                self.floats.push(Block::Group(Group {
                    kind: GroupKind::TextBox,
                    title: None,
                    blocks,
                }));
            }
        }
    }

    /// A picture of the drawing group's picture store.
    fn store_picture(&mut self, index: u32) -> Option<doctopus_core::PartId> {
        let origin = format!("blip:{index}");
        if let Some(part) = self.cx.doc.part_by_origin(&origin) {
            return Some(part.id.clone());
        }
        let max = self.cx.options.max_part_size;
        let blip = self
            .doc
            .drawings
            .store
            .get(index, Some(self.doc.word), max)?;
        let location = self.location();
        Some(self.cx.picture(blip, origin, &location))
    }

    /// The text of a text box shape: the text box story entry whose shape
    /// id matches.
    fn text_box_range(&self, shape: &Shape<'_>) -> Option<(Story, Range<u32>)> {
        let doc = self.doc;
        let (plc, story) = match self.story {
            Story::Main => (&doc.drawings.text_boxes, Story::TextBoxes),
            Story::Headers => (&doc.drawings.header_text_boxes, Story::HeaderTextBoxes),
            _ => return None,
        };
        shape.client_textbox?;
        let i = (0..plc.len()).find(|&i| {
            plc.data(i)
                .and_then(|d| crate::binary::u32_at(d, 14))
                .is_some_and(|lid| lid == shape.id)
        })?;
        Some((story, doc.stories.within(story, plc.range(i)?)))
    }

    fn text_box(&mut self, story: Story, range: Range<u32>) {
        if self.nesting >= MAX_NESTING || !self.text_boxes.insert((range.start, range.end)) {
            return;
        }
        self.nesting += 1;
        let floats = std::mem::take(&mut self.floats);
        let fields = std::mem::take(&mut self.fields);
        let blocks = self.story(story, range);
        self.fields = fields;
        self.floats = floats;
        self.nesting -= 1;
        if blocks.iter().any(|b| !b.is_blank()) {
            self.floats.push(Block::Group(Group {
                kind: GroupKind::TextBox,
                title: None,
                blocks,
            }));
        }
    }
}

fn push_char(text: &mut Vec<u16>, c: char) {
    let mut buf = [0u16; 2];
    text.extend_from_slice(c.encode_utf16(&mut buf));
}

/// Lay out the rows of a table on a grid of column edges: cells spanning
/// several columns, vertical merges, and leading header rows.
fn build_table(rows: Vec<(Vec<Vec<Block>>, Option<RowDef>)>) -> Table {
    let mut edges: Vec<i32> = rows
        .iter()
        .filter_map(|(_, def)| def.as_ref())
        .flat_map(|def| def.cells.iter())
        .flat_map(|c| [i32::from(c.left), i32::from(c.right)])
        .collect();
    edges.sort_unstable();
    edges.dedup_by(|a, b| *a - *b <= EDGE_TOLERANCE);
    let column = |x: i16| {
        edges
            .iter()
            .position(|&e| (e - i32::from(x)).abs() <= EDGE_TOLERANCE)
    };
    let span = |left: i16, right: i16| {
        let (left, right) = (i32::from(left), i32::from(right));
        edges
            .iter()
            .filter(|&&e| e >= left - EDGE_TOLERANCE && e < right - EDGE_TOLERANCE)
            .count()
            .max(1) as u32
    };
    // Each cell's start column and vertical merge, to count row spans.
    let layout: Vec<Vec<(Option<usize>, VMerge)>> = rows
        .iter()
        .map(|(cells, def)| {
            (0..cells.len())
                .map(|c| {
                    let def = def.as_ref().and_then(|d| d.cells.get(c));
                    (
                        def.and_then(|d| column(d.left)),
                        def.map_or(VMerge::None, |d| d.v_merge),
                    )
                })
                .collect()
        })
        .collect();
    let continues = |r: usize, col: usize| {
        layout.get(r).is_some_and(|row| {
            row.iter()
                .any(|&(c, m)| c == Some(col) && m == VMerge::Continue)
        })
    };
    let header_rows = rows
        .iter()
        .take_while(|(_, def)| def.as_ref().is_some_and(|d| d.header))
        .count() as u32;
    let rows = rows
        .into_iter()
        .enumerate()
        .map(|(r, (cells, def))| TableRow {
            cells: cells
                .into_iter()
                .enumerate()
                .map(|(c, blocks)| {
                    let mut cell = TableCell::new(blocks);
                    let Some(d) = def.as_ref().and_then(|d| d.cells.get(c)) else {
                        return cell;
                    };
                    cell.col_span = span(d.left, d.right);
                    cell.merged = d.h_merged || d.v_merge == VMerge::Continue;
                    if d.v_merge == VMerge::Restart
                        && let Some(col) = layout[r][c].0
                    {
                        let mut rows_spanned = 1;
                        while continues(r + rows_spanned, col) {
                            rows_spanned += 1;
                        }
                        cell.row_span = rows_spanned as u32;
                    }
                    cell
                })
                .collect(),
        })
        .collect();
    Table { rows, header_rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy::doc::props::CellDef;

    fn def(edges: &[i16], merges: &[VMerge]) -> RowDef {
        RowDef {
            cells: edges
                .windows(2)
                .zip(merges)
                .map(|(w, &v_merge)| CellDef {
                    left: w[0],
                    right: w[1],
                    h_merged: false,
                    v_merge,
                })
                .collect(),
            header: false,
        }
    }

    #[test]
    fn tables_span_columns_and_rows() {
        let cell = || vec![Block::text("x")];
        let rows = vec![
            (
                vec![cell(), cell()],
                Some(def(&[0, 2000, 4000], &[VMerge::Restart, VMerge::None])),
            ),
            (
                vec![cell(), cell(), cell()],
                Some(def(
                    &[0, 2000, 3000, 4000],
                    &[VMerge::Continue, VMerge::None, VMerge::None],
                )),
            ),
        ];
        let table = build_table(rows);
        assert_eq!(table.rows[0].cells[0].row_span, 2);
        assert_eq!(table.rows[0].cells[1].col_span, 2);
        assert!(table.rows[1].cells[0].merged);
        assert_eq!(table.rows[1].cells[2].col_span, 1);
    }
}
