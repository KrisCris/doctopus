use serde::Serialize;

use super::{CellRange, PartId};

/// A structural unit of content.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    /// A heading; `level` is 1 for the most important.
    Heading(Heading),
    /// A paragraph of running text.
    Paragraph(Paragraph),
    /// One item of a (possibly nested) list.
    ListItem(ListItem),
    /// A table of rich cells (Word, PowerPoint).
    Table(Table),
    /// A sheet of typed cells (Excel).
    Grid(Grid),
    /// A picture.
    Image(ImageRef),
    /// An embedded object or file, rendered inline when it is readable.
    Embed(EmbedRef),
    /// Chart data recovered from the chart's own cache.
    Chart(Chart),
    /// Shapes and connectors (Visio pages, SmartArt).
    Diagram(Diagram),
    /// A display equation.
    Math(Math),
    /// Text kept exactly as written: a plain-text file, code.
    Code(Code),
    /// Footnote, endnote or comment text.
    Note(Note),
    /// A labelled container: speaker notes, a text box, a header.
    Group(Group),
}

impl Block {
    /// A paragraph holding plain text.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Paragraph(Paragraph {
            content: vec![Inline::text(text)],
        })
    }

    /// A paragraph from inlines.
    pub fn paragraph(content: Vec<Inline>) -> Self {
        Self::Paragraph(Paragraph { content })
    }

    /// A heading from inlines; the level is clamped to 1..=6.
    pub fn heading(level: u8, content: Vec<Inline>) -> Self {
        Self::Heading(Heading {
            level: level.clamp(1, 6),
            content,
            anchors: Vec::new(),
        })
    }

    /// True for paragraphs and headings without visible text.
    pub fn is_blank(&self) -> bool {
        match self {
            Self::Paragraph(p) => inlines_are_blank(&p.content),
            Self::Heading(h) => inlines_are_blank(&h.content),
            Self::ListItem(l) => inlines_are_blank(&l.content),
            Self::Group(g) => g.blocks.iter().all(Block::is_blank),
            Self::Code(c) => c.text.trim().is_empty(),
            _ => false,
        }
    }
}

/// A heading.
#[derive(Debug, Clone, Serialize)]
pub struct Heading {
    /// 1 (most important) to 6.
    pub level: u8,
    /// Heading text.
    pub content: Vec<Inline>,
    /// Names that links inside the document reach the heading by: a link
    /// to `#name` leads here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<String>,
}

/// A paragraph.
#[derive(Debug, Clone, Serialize)]
pub struct Paragraph {
    /// Paragraph text.
    pub content: Vec<Inline>,
}

/// One list item. Lists are flat: nesting is expressed by `level`.
#[derive(Debug, Clone, Serialize)]
pub struct ListItem {
    /// Nesting depth, 0 for top level.
    pub level: u8,
    /// The number as the author sees it (`"3."`, `"4.2(b)"`, `"iv)"`);
    /// `None` for bullets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Item text.
    pub content: Vec<Inline>,
}

/// A table whose cells contain blocks.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Table {
    /// Rows, top to bottom.
    pub rows: Vec<TableRow>,
    /// How many leading rows are header rows.
    #[serde(skip_serializing_if = "is_zero")]
    pub header_rows: u32,
}

/// A table row.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TableRow {
    /// Cells, left to right. A cell spanning columns appears once.
    pub cells: Vec<TableCell>,
}

/// A table cell.
#[derive(Debug, Clone, Serialize)]
pub struct TableCell {
    /// Cell content.
    pub blocks: Vec<Block>,
    /// Number of grid columns covered.
    #[serde(skip_serializing_if = "is_one")]
    pub col_span: u32,
    /// Number of rows covered (set on the first cell of a vertical merge).
    #[serde(skip_serializing_if = "is_one")]
    pub row_span: u32,
    /// True for the placeholder cells covered by a vertical merge above.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub merged: bool,
}

impl TableCell {
    /// A plain, unmerged cell.
    pub fn new(blocks: Vec<Block>) -> Self {
        Self {
            blocks,
            col_span: 1,
            row_span: 1,
            merged: false,
        }
    }
}

/// Spreadsheet cells. Only non-empty rows and cells are stored.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Grid {
    /// The used range of the sheet, before any selection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimension: Option<CellRange>,
    /// Non-empty rows in ascending order.
    pub rows: Vec<GridRow>,
    /// Merged cell ranges.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub merged: Vec<CellRange>,
}

impl Grid {
    /// Number of non-empty cells.
    pub fn cell_count(&self) -> usize {
        self.rows.iter().map(|r| r.cells.len()).sum()
    }
}

/// A spreadsheet row.
#[derive(Debug, Clone, Serialize)]
pub struct GridRow {
    /// Zero-based row index (row `1` in Excel is `0`).
    pub row: u32,
    /// Non-empty cells in ascending column order.
    pub cells: Vec<GridCell>,
}

/// A spreadsheet cell.
#[derive(Debug, Clone, Serialize)]
pub struct GridCell {
    /// Zero-based column index (column `A` is `0`).
    pub col: u32,
    /// Cached value.
    pub value: Value,
    /// Formula without the leading `=`, when the cell has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// Hyperlink target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Cell note or comment text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

impl GridCell {
    /// A cell holding just a value.
    pub fn new(col: u32, value: Value) -> Self {
        Self {
            col,
            value,
            formula: None,
            link: None,
            comment: None,
        }
    }
}

/// A typed cell value.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    /// No value (a cell that only carries a formula, comment or link).
    Empty,
    /// Text.
    Text(String),
    /// A number. Rendered with at most 15 significant digits, like Excel.
    Number(f64),
    /// TRUE or FALSE.
    Bool(bool),
    /// A date and/or time in ISO 8601 (`2024-05-01`, `2024-05-01T13:45:00`).
    DateTime(String),
    /// An error such as `#DIV/0!`.
    Error(String),
}

impl Value {
    /// The value as display text.
    pub fn display(&self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Text(s) | Self::DateTime(s) | Self::Error(s) => s.clone(),
            Self::Number(n) => format_number(*n),
            Self::Bool(true) => "TRUE".into(),
            Self::Bool(false) => "FALSE".into(),
        }
    }
}

/// Format a number the way a spreadsheet's "General" format would, but
/// without losing integer digits: integers print exactly, other values keep
/// 15 significant digits, extreme magnitudes use scientific notation.
pub fn format_number(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if n == 0.0 {
        return "0".into();
    }
    let abs = n.abs();
    if n.fract() == 0.0 && abs < 1e16 {
        // Exact: integers below 1e16 fit in an i64.
        return format!("{}", n as i64);
    }
    if !(1e-9..1e16).contains(&abs) {
        let s = format!("{n:.14e}");
        let (mantissa, exponent) = s.split_once('e').unwrap_or((&s, "0"));
        let mantissa = trim_fraction(mantissa);
        let exponent: i32 = exponent.parse().unwrap_or(0);
        let sign = if exponent < 0 { '-' } else { '+' };
        return format!("{mantissa}E{sign}{:02}", exponent.abs());
    }
    let int_digits = abs.log10().floor() as i32 + 1;
    let decimals = (15 - int_digits).clamp(0, 24) as usize;
    trim_fraction(&format!("{n:.decimals$}")).to_string()
}

fn trim_fraction(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// A reference to an image part.
#[derive(Debug, Clone, Serialize)]
pub struct ImageRef {
    /// The image part.
    pub part: PartId,
    /// Alternative text or caption.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
}

/// A reference to an embedded object or file part.
#[derive(Debug, Clone, Serialize)]
pub struct EmbedRef {
    /// The embedded part.
    pub part: PartId,
}

/// Chart data.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Chart {
    /// Chart title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Chart type(s), e.g. `"bar"`, `"line + bar"`.
    pub kind: String,
    /// Category labels (x axis).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    /// Data series.
    pub series: Vec<Series>,
}

/// One chart series. `values[i]` belongs to `categories[i]`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Series {
    /// Series name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Values; `None` for gaps.
    pub values: Vec<Option<f64>>,
}

/// Shapes and the connectors between them.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Diagram {
    /// Shapes in document order; groups nest their members.
    pub shapes: Vec<Shape>,
    /// Directed connections between shapes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<Connection>,
}

/// A diagram shape.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Shape {
    /// Identifier unique within the diagram, used by [`Connection`]s.
    pub id: String,
    /// Shape type, typically the stencil master name (`"Process"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Visible text.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Shape data / custom properties.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub data: Vec<(String, String)>,
    /// Hyperlinks attached to the shape.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<String>,
    /// Picture or embedded object shown by the shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<PartId>,
    /// Members of a group shape.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Shape>,
}

impl Shape {
    /// The text the shape shows: its own, or else that of its members,
    /// joined with ` / `. A UML class, for one, writes its name in a member
    /// shape, and an association its role and multiplicities.
    pub fn shown_text(&self) -> String {
        fn members(shapes: &[Shape], out: &mut Vec<String>) {
            for shape in shapes {
                match shape.text.trim() {
                    "" => members(&shape.children, out),
                    text => out.push(text.to_string()),
                }
            }
        }
        match self.text.trim() {
            "" => {
                let mut texts = Vec::new();
                members(&self.children, &mut texts);
                texts.join(" / ")
            }
            text => text.to_string(),
        }
    }
}

/// A directed edge between two shapes.
#[derive(Debug, Clone, Serialize)]
pub struct Connection {
    /// Source shape id.
    pub from: String,
    /// Target shape id.
    pub to: String,
    /// Connector text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The connector shape, when it is also listed among the shapes (because
    /// it carries data of its own).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connector: Option<String>,
    /// Whether the connection was inferred from where the connector ends
    /// rather than recorded in the file.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inferred: bool,
}

/// An equation, as TeX-like linear text.
#[derive(Debug, Clone, Serialize)]
pub struct Math {
    /// The equation.
    pub tex: String,
}

/// Text kept exactly as written, line breaks and spacing included.
#[derive(Debug, Clone, Serialize)]
pub struct Code {
    /// What the text is, as a Markdown code fence names it (`csv`, `json`),
    /// when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// The text, with lines separated by `\n`.
    pub text: String,
}

/// Footnote, endnote or comment text.
#[derive(Debug, Clone, Serialize)]
pub struct Note {
    /// What kind of note.
    pub kind: NoteKind,
    /// Label used by the matching [`NoteRef`] (`"1"`, `"c3"`).
    pub label: String,
    /// Comment author.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Comment date, ISO 8601.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// The text a comment is attached to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// The note's content.
    pub blocks: Vec<Block>,
}

/// Kinds of [`Note`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NoteKind {
    /// Footnote.
    Footnote,
    /// Endnote.
    Endnote,
    /// Review comment.
    Comment,
}

/// A labelled container of blocks.
#[derive(Debug, Clone, Serialize)]
pub struct Group {
    /// What the container is.
    pub kind: GroupKind,
    /// Optional label (header variant, shape name, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Content.
    pub blocks: Vec<Block>,
}

/// Kinds of [`Group`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    /// Speaker notes of a slide.
    Notes,
    /// A text box or shape text outside the main flow.
    TextBox,
    /// A page header.
    Header,
    /// A page footer.
    Footer,
    /// A quotation set off from the text.
    Quote,
    /// Anything else.
    Other,
}

/// Inline content.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Inline {
    /// A run of text with one style.
    Text(Text),
    /// A hyperlink.
    Link(Link),
    /// A manual line break inside a paragraph.
    LineBreak,
    /// A footnote, endnote or comment reference.
    NoteRef(NoteRef),
    /// An inline picture.
    Image(ImageRef),
    /// An inline equation.
    Math(Math),
}

impl Inline {
    /// Unstyled text.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(Text {
            text: text.into(),
            style: Style::default(),
        })
    }

    /// Styled text.
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self::Text(Text {
            text: text.into(),
            style,
        })
    }
}

/// Styled text.
#[derive(Debug, Clone, Serialize)]
pub struct Text {
    /// The text.
    pub text: String,
    /// Character formatting that carries meaning.
    #[serde(flatten)]
    pub style: Style,
}

/// Character formatting worth preserving.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Style {
    /// Bold.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    /// Italic.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    /// Struck through.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub strike: bool,
    /// Superscript.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub superscript: bool,
    /// Subscript.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub subscript: bool,
    /// In a fixed-width font: code, a file name, a command.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub code: bool,
}

/// A hyperlink.
#[derive(Debug, Clone, Serialize)]
pub struct Link {
    /// Target URL, or `#bookmark` for internal links.
    pub url: String,
    /// Link text.
    pub content: Vec<Inline>,
}

/// A reference to a [`Note`].
#[derive(Debug, Clone, Serialize)]
pub struct NoteRef {
    /// What kind of note is referenced.
    pub kind: NoteKind,
    /// The note's label.
    pub label: String,
}

/// Append text to a run list, merging with the previous run when the style
/// matches. Keeps readers from producing thousands of one-character runs.
pub fn push_text(content: &mut Vec<Inline>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(Inline::Text(last)) = content.last_mut()
        && last.style == style
    {
        last.text.push_str(text);
        return;
    }
    content.push(Inline::styled(text, style));
}

/// Concatenate the visible text of inlines (links contribute their text,
/// line breaks become `\n`, pictures and note references are skipped).
pub fn plain_text(content: &[Inline]) -> String {
    let mut out = String::new();
    append_plain_text(content, &mut out);
    out
}

fn append_plain_text(content: &[Inline], out: &mut String) {
    for inline in content {
        match inline {
            Inline::Text(t) => out.push_str(&t.text),
            Inline::Link(l) => append_plain_text(&l.content, out),
            Inline::LineBreak => out.push('\n'),
            Inline::Math(m) => out.push_str(&m.tex),
            Inline::NoteRef(_) | Inline::Image(_) => {}
        }
    }
}

/// Concatenate the visible text of blocks, one line per paragraph.
pub fn blocks_plain_text(blocks: &[Block]) -> String {
    let mut lines = Vec::new();
    for block in blocks {
        match block {
            Block::Heading(h) => lines.push(plain_text(&h.content)),
            Block::Paragraph(p) => lines.push(plain_text(&p.content)),
            Block::ListItem(l) => lines.push(plain_text(&l.content)),
            Block::Group(g) => lines.push(blocks_plain_text(&g.blocks)),
            Block::Note(n) => lines.push(blocks_plain_text(&n.blocks)),
            Block::Table(t) => {
                for row in &t.rows {
                    let cells: Vec<String> = row
                        .cells
                        .iter()
                        .map(|c| blocks_plain_text(&c.blocks))
                        .collect();
                    lines.push(cells.join("\t"));
                }
            }
            Block::Math(m) => lines.push(m.tex.clone()),
            Block::Code(c) => lines.push(c.text.clone()),
            _ => {}
        }
    }
    lines.retain(|l| !l.trim().is_empty());
    lines.join("\n")
}

fn inlines_are_blank(content: &[Inline]) -> bool {
    content.iter().all(|i| match i {
        Inline::Text(t) => t.text.trim().is_empty(),
        Inline::Link(l) => inlines_are_blank(&l.content),
        Inline::LineBreak => true,
        Inline::NoteRef(_) | Inline::Image(_) | Inline::Math(_) => false,
    })
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_format_like_a_spreadsheet() {
        assert_eq!(format_number(0.1 + 0.2), "0.3");
        assert_eq!(format_number(42.0), "42");
        assert_eq!(format_number(-0.0), "0");
        assert_eq!(format_number(123.456), "123.456");
        assert_eq!(format_number(1234567890123.0), "1234567890123");
        assert_eq!(format_number(0.000123), "0.000123");
        assert_eq!(format_number(1.5e20), "1.5E+20");
        assert_eq!(format_number(2.5e-12), "2.5E-12");
        assert_eq!(format_number(1.0 / 3.0), "0.333333333333333");
        assert_eq!(format_number(-12.75), "-12.75");
    }

    #[test]
    fn push_text_merges_runs_with_equal_style() {
        let mut content = Vec::new();
        push_text(&mut content, "Hello", Style::default());
        push_text(&mut content, ", world", Style::default());
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        push_text(&mut content, "!", bold);
        assert_eq!(content.len(), 2);
        assert_eq!(plain_text(&content), "Hello, world!");
    }
}
