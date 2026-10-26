use super::*;
use crate::{
    Address, Block, Document, DocumentKind, FormatInfo, Grid, GridCell, GridRow, GroupKind, Inline,
    Note, Part, PartKind, Section, SectionKind, Style, TableCell, TableRow, Value,
};

static TEST_FORMAT: FormatInfo = FormatInfo {
    id: "test",
    name: "Test document",
    kind: DocumentKind::Text,
    extensions: &["test"],
    mime: "application/x-test",
};

fn options() -> RenderOptions {
    RenderOptions::new(Address::parse_with("report.test", |_| true))
}

fn bold(text: &str) -> Inline {
    Inline::styled(
        text,
        Style {
            bold: true,
            ..Style::default()
        },
    )
}

#[test]
fn renders_body_with_front_matter_tables_and_notes() {
    let mut doc = Document::new(&TEST_FORMAT);
    doc.metadata.set("title", "Quarterly: report");
    let mut body = Section::new(SectionKind::Body);
    body.blocks
        .push(Block::heading(1, vec![Inline::text("Overview")]));
    body.blocks.push(Block::paragraph(vec![
        Inline::text("Revenue grew "),
        bold(" 12% "),
        Inline::text("this quarter"),
        Inline::NoteRef(crate::NoteRef {
            kind: crate::NoteKind::Footnote,
            label: "1".into(),
        }),
        Inline::text("."),
    ]));
    body.blocks.push(Block::ListItem(crate::ListItem {
        level: 0,
        label: None,
        content: vec![Inline::text("first")],
    }));
    body.blocks.push(Block::ListItem(crate::ListItem {
        level: 1,
        label: Some("a)".into()),
        content: vec![Inline::text("nested")],
    }));
    body.blocks.push(Block::Table(crate::Table {
        rows: vec![
            TableRow {
                cells: vec![
                    TableCell::new(vec![Block::text("Region")]),
                    TableCell::new(vec![Block::text("Q1")]),
                ],
            },
            TableRow {
                cells: vec![
                    TableCell::new(vec![Block::text("North | East")]),
                    TableCell::new(vec![Block::text("1"), Block::text("2")]),
                ],
            },
        ],
        header_rows: 1,
    }));
    doc.sections.push(body);
    let mut notes = Section::new(SectionKind::Footnotes);
    notes.blocks.push(Block::Note(Note {
        kind: crate::NoteKind::Footnote,
        label: "1".into(),
        author: None,
        date: None,
        anchor: None,
        blocks: vec![Block::text("Unaudited.")],
    }));
    doc.sections.push(notes);

    let out = markdown(&doc, &options()).text;
    insta::assert_snapshot!(out, @r#"
    ---
    source: report.test
    format: test (Test document)
    title: "Quarterly: report"
    ---

    # Overview

    Revenue grew **12%** this quarter[^1].

    - first
      a) nested

    | Region | Q1 |
    |---|---|
    | North \| East | 1<br>2 |

    ## Footnotes

    [^1]: Unaudited.
    "#);
}

#[test]
fn renders_grids_with_coordinates_and_embedded_documents_inline() {
    let mut inner = Document::new(&TEST_FORMAT);
    let mut sheet = Section::new(SectionKind::Sheet)
        .with_number(1)
        .with_name("Data");
    sheet.blocks.push(Block::Grid(Grid {
        dimension: None,
        rows: vec![
            GridRow {
                row: 0,
                cells: vec![
                    GridCell::new(0, Value::Text("Item".into())),
                    GridCell::new(2, Value::Text("Cost".into())),
                ],
            },
            GridRow {
                row: 3,
                cells: vec![
                    GridCell::new(0, Value::Text("Pens".into())),
                    GridCell::new(2, Value::Number(0.1 + 0.2)),
                ],
            },
        ],
        merged: Vec::new(),
    }));
    inner.sections.push(sheet);

    let mut doc = Document::new(&TEST_FORMAT);
    let mut part = Part::new(PartKind::Object, vec![0; 2048]).with_name("Budget.test");
    part.document = Some(Box::new(inner));
    let id = doc.add_part(part);
    let image = doc.add_part(
        Part::new(PartKind::Image, vec![0; 10])
            .with_content_type("image/png")
            .with_origin("media/a.png"),
    );
    let mut body = Section::new(SectionKind::Body);
    body.blocks.push(Block::Embed(crate::EmbedRef { part: id }));
    body.blocks.push(Block::Group(crate::Group {
        kind: GroupKind::TextBox,
        title: None,
        blocks: vec![Block::text("Boxed text")],
    }));
    doc.sections.push(body);
    let _ = image;

    let mut opts = options();
    opts.front_matter = false;
    let out = markdown(&doc, &opts).text;
    insta::assert_snapshot!(out, @r#"
    <embedded id="emb1" ref="report.test#emb1" type="test" name="Budget.test" size="2.0 KB">

    ### Sheet 1: Data

    |  | A | B | C |
    |---|---|---|---|
    | 1 | Item |  | Cost |
    | 4 | Pens |  | 0.3 |

    </embedded>

    > Boxed text

    ## Other embedded content

    ![image](report.test#img1 "PNG, 10 B")
    "#);
}

#[test]
fn records_section_offsets_in_characters() {
    let mut doc = Document::new(&TEST_FORMAT);
    for n in 1..=2 {
        let mut s = Section::new(SectionKind::Slide)
            .with_number(n)
            .with_name("Ünïcode");
        s.blocks.push(Block::text("é".repeat(10)));
        doc.sections.push(s);
    }
    let mut opts = options();
    opts.front_matter = false;
    let rendered = markdown(&doc, &opts);
    assert_eq!(rendered.marks.len(), 2);
    let second = rendered.marks[1].offset;
    let tail: String = rendered.text.chars().skip(second).take(10).collect();
    assert_eq!(tail, "## Slide 2");
}

/// A sheet holding "Top" in A1 and 42 in the second row at column `col`.
fn two_cell_sheet(col: u32) -> Document {
    let mut sheet = Section::new(SectionKind::Sheet).with_number(1);
    sheet.blocks.push(Block::Grid(Grid {
        dimension: None,
        rows: vec![
            GridRow {
                row: 0,
                cells: vec![GridCell::new(0, Value::Text("Top".into()))],
            },
            GridRow {
                row: 1,
                cells: vec![GridCell::new(col, Value::Number(42.0))],
            },
        ],
        merged: Vec::new(),
    }));
    let mut doc = Document::new(&TEST_FORMAT);
    doc.sections.push(sheet);
    doc
}

#[test]
fn sparse_grids_are_listed_by_address() {
    let mut opts = options();
    opts.front_matter = false;
    // Empty rows are left out of tables anyway, so a sheet is sparse when
    // its rows are mostly empty across many columns.
    let narrow = markdown(&two_cell_sheet(26), &opts).text;
    assert!(narrow.contains("| 2 |  |"), "{narrow}");
    insta::assert_snapshot!(markdown(&two_cell_sheet(701), &opts).text, @r"
    ## Sheet 1

    Cells (sparse, listed by address):
    - A1: Top
    - ZZ2: 42
    ");
}
