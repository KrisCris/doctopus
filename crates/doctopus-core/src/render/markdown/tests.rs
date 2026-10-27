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
      - a) nested

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

fn shape(id: &str, text: &str) -> crate::Shape {
    crate::Shape {
        id: id.into(),
        text: text.into(),
        ..crate::Shape::default()
    }
}

#[test]
fn standalone_output_links_files_and_draws_connections() {
    let mut inner = Document::new(&TEST_FORMAT);
    inner.sections.push(Section::new(SectionKind::Body));
    let mut doc = Document::new(&TEST_FORMAT);
    let mut budget = Part::new(PartKind::Object, vec![0; 2048]).with_name("Budget.test");
    budget.document = Some(Box::new(inner));
    let budget = doc.add_part(budget);
    let mut broken = Part::new(PartKind::Object, vec![0; 10])
        .with_name("Broken.test")
        .with_content_type("application/x-test");
    broken.error = Some("malformed".into());
    let broken = doc.add_part(broken);
    let logo = doc.add_part(
        Part::new(PartKind::Image, Vec::new())
            .with_name("logo.png")
            .with_link("https://example.com/logo.png"),
    );

    let mut page = Section::new(SectionKind::Page).with_number(1);
    page.blocks
        .push(Block::Embed(crate::EmbedRef { part: budget }));
    page.blocks
        .push(Block::Embed(crate::EmbedRef { part: broken }));
    page.blocks.push(Block::Image(crate::ImageRef {
        part: logo,
        alt: None,
    }));
    let connection = |from: &str, to: &str, label: Option<&str>, inferred| crate::Connection {
        from: from.into(),
        to: to.into(),
        label: label.map(Into::into),
        connector: None,
        inferred,
    };
    page.blocks.push(Block::Diagram(crate::Diagram {
        shapes: vec![shape("1", "Start"), shape("2", "Say \"hi\" <now> #1")],
        connections: vec![
            connection("1", "2", Some("yes|no"), false),
            connection("2", "1", None, true),
        ],
    }));
    doc.sections.push(page);

    let mut opts = options();
    opts.front_matter = false;
    opts.standalone = true;
    opts.extracted
        .insert("emb1".into(), "report.parts/emb1/Budget.md".into());
    opts.extracted
        .insert("emb2".into(), "report.parts/Broken.test".into());
    insta::assert_snapshot!(markdown(&doc, &opts).text, @r##"
    ## Page 1

    Embedded document: [Budget.test](report.parts/emb1/Budget.md "Test document, 2.0 KB")

    Embedded file: [Broken.test](report.parts/Broken.test "application/x-test, 10 B"); it could not be read: malformed

    ![logo.png](https://example.com/logo.png "linked")

    Shapes:
    - 1 "Start"
    - 2 "Say "hi" <now> #1"

    Connections:
    - 1 "Start" -> 2 "Say "hi" <now> #1" (yes|no)
    - 2 "Say "hi" <now> #1" -> 1 "Start" [inferred from positions]

    ```mermaid
    flowchart TD
        n1["Start"]
        n2["Say #quot;hi#quot; #lt;now#gt; #35;1"]
        n1 -->|"yes#124;no"| n2
        n2 -.-> n1
    ```
    "##);

    // Without standalone, the same document inlines and suggests commands.
    opts.standalone = false;
    let read = markdown(&doc, &opts).text;
    assert!(read.contains("<embedded id=\"emb1\""));
    assert!(!read.contains("```mermaid"));
}

#[test]
fn shapes_without_text_are_named_by_their_members() {
    // A UML class writes its name in a member shape.
    let class = |id: &str, name: &str| crate::Shape {
        kind: Some("Class".into()),
        children: vec![shape(&format!("{id}n"), name)],
        ..shape(id, "")
    };
    let mut page = Section::new(SectionKind::Page).with_number(1);
    page.blocks.push(Block::Diagram(crate::Diagram {
        shapes: vec![
            class("1", "Routes"),
            class("2", "Bus Station"),
            shape("3", ""),
        ],
        connections: vec![crate::Connection {
            from: "1".into(),
            to: "2".into(),
            label: None,
            connector: None,
            inferred: false,
        }],
    }));
    let mut doc = Document::new(&TEST_FORMAT);
    doc.sections.push(page);
    let mut opts = options();
    opts.front_matter = false;
    opts.standalone = true;
    let text = markdown(&doc, &opts).text;
    assert!(
        text.contains("- 1 \"Routes\" -> 2 \"Bus Station\"\n"),
        "{text}"
    );
    assert!(
        text.contains("n1[\"Routes\"]\n    n2[\"Bus Station\"]\n"),
        "{text}"
    );
    assert_eq!(class("4", "").shown_text(), "");
    let named = crate::Shape {
        children: vec![shape("a", "1"), class("b", "Pass"), shape("c", "0..*")],
        ..shape("g", "")
    };
    assert_eq!(named.shown_text(), "1 / Pass / 0..*");
    assert_eq!(shape("own", " Fare ").shown_text(), "Fare");
}

fn item(level: u8, label: Option<&str>, text: &str) -> Block {
    Block::ListItem(crate::ListItem {
        level,
        label: label.map(str::to_string),
        content: vec![Inline::text(text)],
    })
}

#[test]
fn formatting_survives_as_markdown() {
    let code = Style {
        code: true,
        ..Style::default()
    };
    let mut body = Section::new(SectionKind::Body);
    let mut contents = Block::heading(1, vec![Inline::text("Contents")]);
    if let Block::Heading(h) = &mut contents {
        h.anchors = vec!["toc".into()];
    }
    body.blocks.push(contents);
    // A link to a heading further down, and one to nowhere.
    body.blocks.push(Block::paragraph(vec![
        Inline::Link(crate::Link {
            url: "#_Ref2".into(),
            content: vec![Inline::text("Usage")],
        }),
        Inline::text(", "),
        Inline::Link(crate::Link {
            url: "#toc".into(),
            content: vec![Inline::text("top")],
        }),
    ]));
    body.blocks.push(Block::paragraph(vec![
        Inline::text("Run "),
        Inline::styled("ls -l", code),
        Inline::text(" or "),
        Inline::styled("a`b](c)", code),
        Inline::LineBreak,
        Inline::text("next line"),
    ]));
    body.blocks.push(item(0, Some("10."), "ten"));
    body.blocks.push(item(1, None, "under ten"));
    body.blocks.push(item(2, Some("a)"), "lettered"));
    body.blocks.push(item(0, Some("11."), "eleven"));
    body.blocks.push(Block::Group(crate::Group {
        kind: GroupKind::Quote,
        title: None,
        blocks: vec![Block::text("Quoted"), Block::text("twice")],
    }));
    let mut usage = Block::heading(2, vec![Inline::text("Usage: the CLI")]);
    if let Block::Heading(h) = &mut usage {
        h.anchors = vec!["_Ref2".into()];
    }
    body.blocks
        .push(Block::heading(2, vec![Inline::text("Usage: the CLI")]));
    body.blocks.push(usage);
    let mut doc = Document::new(&TEST_FORMAT);
    doc.sections.push(body);
    let mut opts = options();
    opts.front_matter = false;
    insta::assert_snapshot!(markdown(&doc, &opts).text, @r"
    # Contents

    [Usage](#usage-the-cli-1), [top](#contents)

    Run `ls -l` or ``a`b](c)``\
    next line

    10. ten
        - under ten
          - a) lettered
    11. eleven

    > Quoted
    >
    > twice

    ## Usage: the CLI

    ## Usage: the CLI
    ");

    let text = super::text(&doc, &opts).text;
    assert!(text.starts_with("Contents\n\nUsage, top\n"), "{text}");
}

#[test]
fn links_to_slides_find_their_heading() {
    let link = |url: &str, text: &str| {
        Inline::Link(crate::Link {
            url: url.into(),
            content: vec![Inline::text(text)],
        })
    };
    let mut first = Section::new(SectionKind::Slide).with_number(1);
    first.blocks.push(Block::paragraph(vec![
        link("#slide-2", "results"),
        Inline::text(" and "),
        link("#_ftn1", "a note"),
    ]));
    let second = Section::new(SectionKind::Slide)
        .with_number(2)
        .with_name("Results");
    let mut doc = Document::new(&TEST_FORMAT);
    doc.sections.extend([first, second]);
    let mut opts = options();
    opts.front_matter = false;
    let read = markdown(&doc, &opts).text;
    assert!(
        read.contains("[results](#slide-2-results) and [a note](#_ftn1)"),
        "{read}"
    );
    opts.standalone = true;
    let converted = markdown(&doc, &opts).text;
    assert!(
        converted.contains("[results](#slide-2-results) and a note"),
        "{converted}"
    );
}
