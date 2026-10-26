//! Walking WordprocessingML content: paragraphs, runs, fields, tables,
//! drawings and objects, in the body as well as in headers, footers,
//! footnotes, comments and text boxes.

use std::collections::HashMap;

use doctopus_core::{
    Block, EmbedRef, Group, GroupKind, ImageRef, Inline, Link, Math, NoteKind, NoteRef, Style,
    Table, TableCell, TableRow, push_text,
};

use super::styles::{Styles, toggle};
use crate::numbering::{Counters, ListLabel, NumRef, Numbering};
use crate::ooxml::context::Context;
use crate::ooxml::drawing::{self, push_math};
use crate::ooxml::math;
use crate::ooxml::package::Relationships;
use crate::ooxml::xml::Element;
use crate::word::{
    MAX_ANCHOR, hyperlink_target, is_blank, paragraph_block, symbol_char, trim_inlines,
};

/// A field (`w:fldChar` begin ... separate ... end) being read.
#[derive(Debug, Default)]
struct Field {
    instruction: String,
    /// Past `separate`: the runs now are the field's displayed result.
    in_result: bool,
    /// For `HYPERLINK` fields: the target and the result collected so far.
    link: Option<String>,
    result: Vec<Inline>,
}

/// Note labels handed out on first reference, so footnotes are numbered
/// in reading order no matter what ids the file uses.
#[derive(Debug, Default)]
pub(crate) struct NoteLabels {
    pub(crate) footnotes: Vec<(String, String)>,
    pub(crate) endnotes: Vec<(String, String)>,
    pub(crate) comments: Vec<(String, String)>,
}

impl NoteLabels {
    fn label(list: &mut Vec<(String, String)>, id: &str, prefix: &str) -> String {
        if let Some((_, label)) = list.iter().find(|(i, _)| i == id) {
            return label.clone();
        }
        let label = format!("{prefix}{}", list.len() + 1);
        list.push((id.to_string(), label.clone()));
        label
    }

    pub(crate) fn footnote(&mut self, id: &str) -> String {
        Self::label(&mut self.footnotes, id, "")
    }

    pub(crate) fn endnote(&mut self, id: &str) -> String {
        Self::label(&mut self.endnotes, id, "e")
    }

    pub(crate) fn comment(&mut self, id: &str) -> String {
        Self::label(&mut self.comments, id, "c")
    }
}

/// Reads WordprocessingML content into blocks.
pub(crate) struct Walker<'c, 'a> {
    pub(crate) cx: &'c mut Context<'a>,
    styles: &'c Styles,
    numbering: &'c Numbering,
    counters: Counters,
    /// Relationships of the part being read.
    pub(crate) rels: Relationships,
    /// Where parts found now are said to be.
    pub(crate) area: String,
    /// Text of the most recent heading, to locate parts.
    heading: Option<String>,
    pub(crate) notes: NoteLabels,
    /// Comments whose anchor range is open, and the text collected for each.
    open_comments: Vec<String>,
    pub(crate) anchors: HashMap<String, String>,
    fields: Vec<Field>,
    /// Blocks met inside the current paragraph (charts, text boxes,
    /// objects) that are emitted after it.
    floats: Vec<Block>,
    pub(crate) insertions: usize,
    pub(crate) deletions: usize,
}

impl<'c, 'a> Walker<'c, 'a> {
    pub(crate) fn new(
        cx: &'c mut Context<'a>,
        styles: &'c Styles,
        numbering: &'c Numbering,
        rels: Relationships,
    ) -> Self {
        Self {
            cx,
            styles,
            numbering,
            counters: Counters::default(),
            rels,
            area: "body".into(),
            heading: None,
            notes: NoteLabels::default(),
            open_comments: Vec::new(),
            anchors: HashMap::new(),
            fields: Vec::new(),
            floats: Vec::new(),
            insertions: 0,
            deletions: 0,
        }
    }

    fn location(&self) -> String {
        match &self.heading {
            Some(h) if self.area == "body" => format!("under \"{h}\""),
            _ => self.area.clone(),
        }
    }

    /// Read block-level content (`w:body`, `w:tc`, `w:txbxContent`, ...).
    pub(crate) fn blocks(&mut self, container: &Element, out: &mut Vec<Block>) {
        for child in container.children() {
            match child.name() {
                "w:p" => self.paragraph(child, out),
                "w:tbl" => {
                    let table = self.table(child);
                    out.push(Block::Table(table));
                }
                "w:sdt" => {
                    if let Some(content) = child.child("w:sdtContent") {
                        self.blocks(content, out);
                    }
                }
                "w:customXml" | "w:ins" | "w:moveTo" | "w:smartTag" => self.blocks(child, out),
                "w:altChunk" => {
                    if let Some(id) = child.attr("r:id") {
                        let location = self.location();
                        if let Some(part) = self.cx.object(&self.rels, id, None, &location) {
                            out.push(Block::Embed(EmbedRef { part }));
                        }
                    }
                }
                "m:oMathPara" | "m:oMath" => {
                    let tex = math::to_tex(child);
                    if !tex.is_empty() {
                        out.push(Block::Math(Math { tex }));
                    }
                }
                _ => {}
            }
        }
    }

    fn paragraph(&mut self, p: &Element, out: &mut Vec<Block>) {
        let ppr = p.child("w:pPr");
        let style = self
            .styles
            .effective(ppr.and_then(|p| p.child_val("w:pStyle")));
        let heading = ppr
            .and_then(|p| p.child_val("w:outlineLvl"))
            .and_then(|v| v.parse::<u8>().ok())
            .map(|o| (o < 9).then_some(o + 1))
            .unwrap_or_else(|| style.and_then(|s| self.styles.heading_level(s)));
        let num = ppr
            .and_then(|p| p.child("w:numPr"))
            .and_then(NumRef::parse)
            .map(|mut direct| {
                if direct.num_id.is_empty()
                    && let Some(from_style) = style.and_then(|s| self.styles.numbering(s))
                {
                    direct.num_id = from_style.num_id;
                }
                direct
            })
            .or_else(|| style.and_then(|s| self.styles.numbering(s)))
            .filter(|n| !n.num_id.is_empty() && n.num_id != "0");

        let mut content = Vec::new();
        self.inlines(p, &mut content);
        self.close_paragraph_fields(&mut content);
        trim_inlines(&mut content);
        let floats = std::mem::take(&mut self.floats);

        let label = match &num {
            Some(num) => self.counters.next(self.numbering, num),
            None => ListLabel::None,
        };
        if !is_blank(&content) {
            let math_only = content.len() == 1
                && matches!(content[0], Inline::Math(_))
                && p.children().any(|c| c.is("m:oMathPara"));
            if math_only {
                if let Some(Inline::Math(m)) = content.pop() {
                    out.push(Block::Math(m));
                }
            } else {
                let level = num.as_ref().and_then(|n| n.level).unwrap_or(0);
                let (block, heading) = paragraph_block(content, heading, label, level);
                if heading.is_some() {
                    self.heading = heading;
                }
                out.push(block);
            }
        }
        out.extend(floats);
    }

    fn table(&mut self, tbl: &Element) -> Table {
        struct Raw {
            start: u32,
            span: u32,
            merge: Merge,
            blocks: Vec<Block>,
        }
        #[derive(PartialEq)]
        enum Merge {
            None,
            Restart,
            Continue,
        }
        let mut rows: Vec<Vec<Raw>> = Vec::new();
        let mut header_rows = 0;
        let mut in_header = true;
        for tr in rows_of(tbl) {
            let trpr = tr.child("w:trPr");
            let is_header = trpr
                .and_then(|p| p.child("w:tblHeader"))
                .is_some_and(toggle);
            if in_header && is_header {
                header_rows += 1;
            } else {
                in_header = false;
            }
            let mut col = trpr
                .and_then(|p| p.child_val("w:gridBefore"))
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0);
            let mut cells = Vec::new();
            for tc in cells_of(tr) {
                let tcpr = tc.child("w:tcPr");
                let span = tcpr
                    .and_then(|p| p.child_val("w:gridSpan"))
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(1)
                    .max(1);
                let merge = match tcpr.and_then(|p| p.child("w:vMerge")) {
                    Some(m) if m.attr("w:val") == Some("restart") => Merge::Restart,
                    Some(_) => Merge::Continue,
                    None if tcpr
                        .and_then(|p| p.child("w:hMerge"))
                        .is_some_and(|m| m.attr("w:val") != Some("restart")) =>
                    {
                        Merge::Continue
                    }
                    None => Merge::None,
                };
                let mut blocks = Vec::new();
                self.blocks(tc, &mut blocks);
                cells.push(Raw {
                    start: col,
                    span,
                    merge,
                    blocks,
                });
                col += span;
            }
            rows.push(cells);
        }
        let continues = |r: usize, start: u32, rows: &Vec<Vec<Raw>>| {
            rows.get(r).is_some_and(|row| {
                row.iter()
                    .any(|c| c.start == start && c.merge == Merge::Continue)
            })
        };
        let mut spans: Vec<Vec<u32>> = Vec::with_capacity(rows.len());
        for (r, row) in rows.iter().enumerate() {
            spans.push(
                row.iter()
                    .map(|cell| {
                        if cell.merge != Merge::Restart {
                            return 1;
                        }
                        let mut span = 1;
                        while continues(r + span as usize, cell.start, &rows) {
                            span += 1;
                        }
                        span
                    })
                    .collect(),
            );
        }
        let rows = rows
            .into_iter()
            .zip(spans)
            .map(|(row, spans)| TableRow {
                cells: row
                    .into_iter()
                    .zip(spans)
                    .map(|(raw, row_span)| {
                        let mut cell = TableCell::new(raw.blocks);
                        cell.col_span = raw.span;
                        cell.row_span = row_span;
                        cell.merged = raw.merge == Merge::Continue;
                        cell
                    })
                    .collect(),
            })
            .collect();
        Table { rows, header_rows }
    }

    /// Read inline content (`w:p`, `w:hyperlink`, `w:ins`, ...).
    fn inlines(&mut self, el: &Element, content: &mut Vec<Inline>) {
        for child in el.children() {
            match child.name() {
                "w:r" => self.run(child, content),
                "w:hyperlink" => {
                    let url = child
                        .attr("r:id")
                        .and_then(|id| Context::hyperlink(&self.rels, id));
                    let mut inner = Vec::new();
                    self.inlines(child, &mut inner);
                    match url {
                        Some(url) if !is_blank(&inner) => content.push(Inline::Link(Link {
                            url,
                            content: inner,
                        })),
                        _ => content.extend(inner),
                    }
                }
                "w:fldSimple" => {
                    let url = child.attr("w:instr").and_then(hyperlink_target);
                    let mut inner = Vec::new();
                    self.inlines(child, &mut inner);
                    match url {
                        Some(url) if !is_blank(&inner) => content.push(Inline::Link(Link {
                            url,
                            content: inner,
                        })),
                        _ => content.extend(inner),
                    }
                }
                "w:ins" | "w:moveTo" => {
                    self.insertions += 1;
                    self.inlines(child, content);
                }
                "w:del" | "w:moveFrom" => self.deletions += 1,
                "w:smartTag" | "w:customXml" | "w:dir" | "w:bdo" | "w:sdtContent" => {
                    self.inlines(child, content)
                }
                "w:sdt" => {
                    if let Some(c) = child.child("w:sdtContent") {
                        self.inlines(c, content);
                    }
                }
                "m:oMath" | "m:oMathPara" => push_math(content, math::to_tex(child)),
                "w:commentRangeStart" => {
                    if let Some(id) = child.attr("w:id") {
                        self.open_comments.push(id.to_string());
                        self.anchors.entry(id.to_string()).or_default();
                    }
                }
                "w:commentRangeEnd" => {
                    if let Some(id) = child.attr("w:id") {
                        self.open_comments.retain(|c| c != id);
                    }
                }
                _ => {}
            }
        }
    }

    fn run(&mut self, r: &Element, content: &mut Vec<Inline>) {
        let rpr = r.child("w:rPr");
        if rpr.and_then(|p| p.child("w:vanish")).is_some_and(toggle) {
            return;
        }
        let style = self.run_style(rpr);
        for child in r.children() {
            match child.name() {
                "w:t" => self.text(&child.text(), style, content),
                "w:tab" | "w:ptab" => self.text(" ", style, content),
                "w:noBreakHyphen" => self.text("-", style, content),
                "w:sym" => {
                    if let Some(c) = symbol(child) {
                        self.text(&c.to_string(), style, content);
                    }
                }
                "w:br" => match child.attr("w:type") {
                    Some("page") | Some("column") => {}
                    _ => self.sink(content).push(Inline::LineBreak),
                },
                "w:cr" => self.sink(content).push(Inline::LineBreak),
                "w:instrText" => {
                    if let Some(field) = self.fields.last_mut().filter(|f| !f.in_result) {
                        field.instruction.push_str(&child.text());
                    }
                }
                "w:fldChar" => self.field_char(child, content),
                "w:footnoteReference" => {
                    if let Some(id) = child.attr("w:id") {
                        let label = self.notes.footnote(id);
                        self.sink(content).push(Inline::NoteRef(NoteRef {
                            kind: NoteKind::Footnote,
                            label,
                        }));
                    }
                }
                "w:endnoteReference" => {
                    if let Some(id) = child.attr("w:id") {
                        let label = self.notes.endnote(id);
                        self.sink(content).push(Inline::NoteRef(NoteRef {
                            kind: NoteKind::Endnote,
                            label,
                        }));
                    }
                }
                "w:commentReference" => {
                    if let Some(id) = child.attr("w:id") {
                        let label = self.notes.comment(id);
                        self.sink(content).push(Inline::NoteRef(NoteRef {
                            kind: NoteKind::Comment,
                            label,
                        }));
                    }
                }
                "w:drawing" => self.drawing(child, content),
                "w:pict" => self.vml(child, content),
                "w:object" => self.object(child, content),
                "w:ruby" => {
                    if let Some(base) = child.child("w:rubyBase") {
                        self.inlines(base, content);
                    }
                }
                "w:delText" => {}
                _ => {}
            }
        }
    }

    fn run_style(&self, rpr: Option<&Element>) -> Style {
        let Some(rpr) = rpr else {
            return Style::default();
        };
        let (style_bold, style_italic) = rpr
            .child_val("w:rStyle")
            .map(|id| self.styles.character(id))
            .unwrap_or_default();
        let vert = rpr.child_val("w:vertAlign");
        Style {
            bold: rpr.child("w:b").map(toggle).or(style_bold).unwrap_or(false),
            italic: rpr
                .child("w:i")
                .map(toggle)
                .or(style_italic)
                .unwrap_or(false),
            strike: rpr.child("w:strike").is_some_and(toggle)
                || rpr.child("w:dstrike").is_some_and(toggle),
            superscript: vert == Some("superscript"),
            subscript: vert == Some("subscript"),
        }
    }

    /// Where run content goes: into a hyperlink field being collected, or
    /// straight into the paragraph.
    fn sink<'s>(&'s mut self, content: &'s mut Vec<Inline>) -> &'s mut Vec<Inline> {
        match self.fields.last_mut() {
            Some(field) if field.in_result && field.link.is_some() => &mut field.result,
            _ => content,
        }
    }

    fn text(&mut self, text: &str, style: Style, content: &mut Vec<Inline>) {
        // Field instructions are not displayed. Only the innermost field
        // counts, so a damaged document with fields that never reach their
        // result does not hide the rest of the text.
        if self.fields.last().is_some_and(|f| !f.in_result) {
            return;
        }
        let text: String = text
            .chars()
            .filter(|c| !('\u{F000}'..='\u{F0FF}').contains(c))
            .collect();
        for id in &self.open_comments {
            if let Some(anchor) = self.anchors.get_mut(id)
                && anchor.chars().count() < MAX_ANCHOR
            {
                anchor.push_str(&text);
            }
        }
        push_text(self.sink(content), &text, style);
    }

    fn field_char(&mut self, el: &Element, content: &mut Vec<Inline>) {
        match el.attr("w:fldCharType") {
            Some("begin") => self.fields.push(Field::default()),
            Some("separate") => {
                if let Some(field) = self.fields.last_mut() {
                    field.in_result = true;
                    field.link = hyperlink_target(&field.instruction);
                }
            }
            Some("end") => {
                let Some(field) = self.fields.pop() else {
                    return;
                };
                if let Some(url) = field.link
                    && !is_blank(&field.result)
                {
                    self.sink(content).push(Inline::Link(Link {
                        url,
                        content: field.result,
                    }));
                }
            }
            _ => {}
        }
    }

    /// Fields may span paragraphs (a table of contents does); a hyperlink
    /// still open at the end of a paragraph gives up its link and keeps its
    /// text.
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

    /// `w:drawing`: pictures inline, everything else after the paragraph.
    fn drawing(&mut self, el: &Element, content: &mut Vec<Inline>) {
        for frame in el
            .children()
            .filter(|c| c.is("wp:inline") || c.is("wp:anchor"))
        {
            let Some(data) = frame.find("a:graphicData") else {
                continue;
            };
            let alt = drawing::alt_text(frame);
            for item in data.children() {
                self.drawing_item(item, alt.clone(), content);
            }
        }
    }

    fn drawing_item(&mut self, item: &Element, alt: Option<String>, content: &mut Vec<Inline>) {
        let location = self.location();
        match item.name() {
            "pic:pic" => match drawing::picture(self.cx, &self.rels, item, &location) {
                Some(Block::Image(mut image)) => {
                    image.alt = image.alt.or(alt);
                    self.sink(content).push(Inline::Image(image));
                }
                Some(other) => self.floats.push(other),
                None => {}
            },
            "wps:wsp" => {
                if let Some(text) = item
                    .child("wps:txbx")
                    .and_then(|t| t.child("w:txbxContent"))
                {
                    self.text_box(text);
                }
            }
            "wpg:wgp" | "wpg:grpSp" | "wpc:wpc" | "wpc:bg" | "wpc:whole" => {
                for child in item.children() {
                    self.drawing_item(child, None, content);
                }
            }
            "c:chart" | "cx:chart" | "dgm:relIds" => {
                let mut blocks = Vec::new();
                drawing::graphic_item(self.cx, &self.rels, item, &location, &mut blocks);
                self.floats.extend(blocks);
            }
            _ => {}
        }
    }

    fn text_box(&mut self, content: &Element) {
        let mut blocks = Vec::new();
        let floats = std::mem::take(&mut self.floats);
        let fields = std::mem::take(&mut self.fields);
        self.blocks(content, &mut blocks);
        self.fields = fields;
        self.floats = floats;
        if blocks.iter().any(|b| !b.is_blank()) {
            self.floats.push(Block::Group(Group {
                kind: GroupKind::TextBox,
                title: None,
                blocks,
            }));
        }
    }

    /// Legacy VML drawings (`w:pict`): images and text boxes.
    fn vml(&mut self, el: &Element, content: &mut Vec<Inline>) {
        for item in el.children() {
            if item.name().starts_with("v:") {
                self.vml_shape(item, content);
            }
        }
    }

    fn vml_shape(&mut self, shape: &Element, content: &mut Vec<Inline>) {
        if shape.is("v:group") {
            for child in shape.children() {
                self.vml_shape(child, content);
            }
            return;
        }
        if let Some(text) = shape
            .child("v:textbox")
            .and_then(|t| t.child("w:txbxContent"))
        {
            self.text_box(text);
        }
        if let Some(id) = shape
            .child("v:imagedata")
            .and_then(|i| i.attr("r:id").or_else(|| i.attr("r:href")))
        {
            let location = self.location();
            if let Some(part) = self.cx.image(&self.rels, id, &location) {
                let alt = shape
                    .attr("alt")
                    .or_else(|| shape.child("v:imagedata").and_then(|i| i.attr("o:title")))
                    .map(str::to_string)
                    .filter(|s| !s.trim().is_empty());
                self.sink(content)
                    .push(Inline::Image(ImageRef { part, alt }));
            }
        }
    }

    /// `w:object`: an embedded or linked OLE object. Its preview picture is
    /// used only when the object itself cannot be found.
    fn object(&mut self, el: &Element, content: &mut Vec<Inline>) {
        let location = self.location();
        let ole = el.descendants().find_map(|e| match e.name() {
            "o:OLEObject" => Some((e.attr("r:id"), e.attr("ProgID"))),
            "w:objectEmbed" | "w:objectLink" => Some((e.attr("r:id"), e.attr("w:progId"))),
            _ => None,
        });
        if let Some((Some(id), prog_id)) = ole
            && let Some(part) = self.cx.object(&self.rels, id, prog_id, &location)
        {
            self.floats.push(Block::Embed(EmbedRef { part }));
            return;
        }
        if let Some(drawing) = el.child("w:drawing") {
            self.drawing(drawing, content);
        } else {
            self.vml(el, content);
        }
    }
}

/// Rows of a table, including rows wrapped in content controls.
fn rows_of(tbl: &Element) -> Vec<&Element> {
    let mut rows = Vec::new();
    collect(tbl, "w:tr", &mut rows);
    rows
}

/// Cells of a row, including cells wrapped in content controls.
fn cells_of(tr: &Element) -> Vec<&Element> {
    let mut cells = Vec::new();
    collect(tr, "w:tc", &mut cells);
    cells
}

fn collect<'e>(el: &'e Element, name: &str, out: &mut Vec<&'e Element>) {
    for child in el.children() {
        if child.is(name) {
            out.push(child);
        } else if matches!(
            child.name(),
            "w:sdt" | "w:sdtContent" | "w:customXml" | "w:ins" | "w:moveTo"
        ) {
            collect(child, name, out);
        }
    }
}

/// A `w:sym` character.
fn symbol(el: &Element) -> Option<char> {
    let code = u32::from_str_radix(el.attr("w:char")?.trim(), 16).ok()?;
    symbol_char(el.attr("w:font").unwrap_or_default(), code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_font_maps_to_unicode() {
        let el = |font: &str, code: &str| {
            let xml = format!(
                r#"<w:sym xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" w:font="{font}" w:char="{code}"/>"#
            );
            crate::ooxml::xml::parse(xml.as_bytes()).root
        };
        assert_eq!(symbol(&el("Symbol", "F061")), Some('α'));
        assert_eq!(symbol(&el("Wingdings", "F0FC")), Some('✓'));
        assert_eq!(symbol(&el("Arial", "2014")), Some('—'));
    }
}
