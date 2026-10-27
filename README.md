# doctopus

**Read Word, Excel, PowerPoint, Visio and PDF files, and everything
embedded in them, as Markdown that coding agents can work with.**

doctopus is a single self-contained binary for Linux, macOS and Windows.
It turns `.docx`, `.doc`, `.xlsx`, `.xls`, `.xlsb`, `.pptx`, `.ppt`, `.vsdx`,
`.vsd` and `.pdf` files into structured Markdown, plain text or JSON.
Embedded content comes along: a workbook pasted into a slide, the document
inside that workbook, a PDF attached to it, and every image and
attachment. Each one gets an address
that other commands accept.

It is built for LLM coding agents such as Claude Code and Codex, and works
just as well for people:

- **Paged output.** A call prints at most 25,000 characters. A note gives
  the exact command for the next page, so a large workbook cannot flood a
  context window.
- **An overview first.** `doctopus info` shows the sheets, slides or pages,
  the embedded parts, and the size of the full output in tokens before you
  read anything.
- **Errors that say what to do next.** They list the sheet names or part
  ids that exist and suggest a command. Exit codes tell failure kinds apart.
- **Deterministic output.** The same file always renders the same way, so
  pages line up and diffs are meaningful.
- **No runtime dependencies.** No Office, LibreOffice, Python, Java or PDF
  library. Linux binaries are fully static.

## Example

```console
$ doctopus info deck.pptx
source: deck.pptx
format: pptx (PowerPoint presentation (Office Open XML))
size: 198.2 KB
title: The quick brown fox jumps over the lazy dog
author: Nevin Nollop
slides: # 1
  - "1 \"The quick brown fox jumps over the lazy dog\": 2 images, 4 embedded objects"
parts: # 11
  - img1: image/png "image4.png" (189x55, 4.3 KB, slide 1)
  - emb1: Excel workbook (Office Open XML) "Microsoft_Office_Excel_Worksheet1.xlsx" (14.0 KB, slide 1)
    - emb1/img1: image/png "image1.png" (189x55, 4.3 KB, sheet "Sheet1" cell A12)
  - emb2: Word document (Office Open XML) "Microsoft_Office_Word_Document2.docx" (30.6 KB, slide 1)
  ...
rendered: ~2,850 characters (~714 tokens) as Markdown
next:
  - doctopus read deck.pptx  # full content as Markdown
  - doctopus read "deck.pptx#emb1"  # only the embedded xlsx
  - doctopus extract deck.pptx -o deck.parts  # save images and embedded files to disk
```

```console
$ doctopus read deck.pptx --no-front-matter
## Slide 1: The quick brown fox jumps over the lazy dog

![alfresco-logo.png](deck.pptx#img1 "PNG, 189x55, 4.3 KB")

<embedded id="emb1" ref="deck.pptx#emb1" type="xlsx" name="Microsoft_Office_Excel_Worksheet1.xlsx" size="14.0 KB">

### Sheet 1: Sheet1

|  | A | B | C | D | E | F | G | H | I |
|---|---|---|---|---|---|---|---|---|---|
| 1 | The quick brown fox jumps over the lazy dog |  |  |  |  |  |  |  |  |
| 3 | The | quick | brown | fox | jumps | over | the | lazy | dog |
...
```

## Install

**Prebuilt binaries.** Every release has archives for Linux, macOS and
Windows, each on x86_64 and arm64. Download the one for your platform from
the [releases page](https://github.com/KrisCris/doctopus/releases), unpack
it, and put `doctopus` (or `doctopus.exe`) on your `PATH`. The Linux
binaries are statically linked with musl and run on any distribution. The
Windows binaries do not need the Visual C++ runtime.

**From source.** With Rust 1.88 or newer:

```sh
cargo install --locked --git https://github.com/KrisCris/doctopus doctopus
```

In a clone of the repository, `cargo install --locked --path crates/doctopus`
does the same.

### Tell your agent about it

Add a line like this to your agent's instructions (`AGENTS.md`,
`CLAUDE.md`, ...):

```markdown
To read Office and PDF files (.docx .doc .xlsx .xls .xlsb .pptx .ppt .vsdx .vsd
.pdf), use `doctopus`: `doctopus info FILE` for an overview, `doctopus read FILE` for the
content as Markdown. `doctopus guide` explains the rest.
```

## Usage

| Command | What it does |
|---|---|
| `doctopus info FILE...` | Format, properties, sheets, slides or pages, embedded parts, output size, and suggested next commands |
| `doctopus read FILE` | The content as Markdown, a page at a time (`doctopus FILE` is short for this) |
| `doctopus extract FILE -o DIR` | Save every image, embedded document and attachment, nested ones included |
| `doctopus formats` | The supported formats and extensions |
| `doctopus guide` | A usage guide written for agents |

### Addresses

An address names a file or something inside it. Ids come from `info`, the
YAML header of `read`, or the `ref` attributes in the output:

```sh
doctopus read report.docx               # the document
doctopus read "deck.pptx#emb1"          # embedded document emb1 on its own
doctopus extract "deck.pptx#emb1/img2"  # image img2 inside that document
```

Quote addresses that contain `#`, because some shells treat it specially.

### Reading less

```sh
doctopus read book.xlsx --sheet 2                # also --slide, --page; numbers, ranges, names
doctopus read book.xlsx --sheet "Q*" --range A1:F50
doctopus read book.xlsx --formulas               # formulas next to their values
doctopus read deck.pptx --offset 25000           # the next page
doctopus read book.xlsx -o book.md               # everything to a file, plus the line where each sheet starts
doctopus read deck.pptx --extract deck.parts     # save parts too; references point at the saved files
```

`-f text` prints plain text and `-f json` prints the full document model.
`--max-chars` (or `DOCTOPUS_MAX_CHARS`) sets the page size; `0` turns
paging off.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | The file could not be read (missing, damaged) |
| 2 | Invalid arguments: unknown option, no such part, sheet or cell range |
| 3 | Unsupported file type |
| 4 | Encrypted file |

## What is read

| Format | Extensions | Content |
|---|---|---|
| Word | `.docx` `.docm` `.dotx` `.dotm` `.doc` `.dot` | Text with headings, lists, tables, links and text boxes; equations as LaTeX; footnotes, endnotes, comments, headers and footers; images, charts, SmartArt, embedded objects |
| Excel | `.xlsx` `.xlsm` `.xltx` `.xltm` `.xlam` `.xlsb` `.xls` `.xlt` `.xla` | Every sheet as a table with column letters and row numbers (sparse sheets cell by cell); formulas, merged cells, hyperlinks, notes and threaded comments, headers and footers, charts, pictures, text boxes, embedded objects |
| PowerPoint | `.pptx` `.pptm` `.ppsx` `.ppsm` `.potx` `.potm` `.ppt` `.pps` `.pot` | Slides in order with titles, text, tables, pictures, charts, SmartArt and embedded objects; speaker notes and comments |
| Visio | `.vsdx` `.vsdm` `.vssx` `.vssm` `.vstx` `.vstm` `.vsd` `.vss` `.vst` | Pages of shapes with their text, shape data and hyperlinks; groups; the connections between shapes; pictures and embedded objects |
| PDF | `.pdf` | Pages with their text in reading order as paragraphs, headings and lists; links; pictures where they appear; bookmarks, page labels, form field values and attached files. Pages that are only pictures (scans) are reported |

Document properties, including custom ones, are read for every format.
Charts are shown as their data. Images are referenced and can be extracted,
so an agent that can view images can open them.

### Limitations

- Encrypted and password-protected files are recognized and reported, not
  decrypted. PDF files that open without a password (those that only
  restrict printing or copying) are read.
- Word 6.0/95 documents and Excel 2.x-4.0 worksheets are not supported.
  Excel 95 workbooks are read for their cells only.
- Visio 4 and older are not supported. Visio 5 files record no shape data,
  hyperlinks or glue, so their connections are inferred from where the
  connector ends touch shapes.
- Formulas are not recalculated. Values are the results the file stores.
- PDF text is rebuilt from where the page draws it, so tables come out as
  paragraphs and unusual layouts can come out in drawing order. Text in
  scanned pages is not recognized (there is no OCR): those pages are
  listed in a warning with their pictures, which an agent that can view
  images can extract and look at. Glyphs whose fonts do not say which
  characters they are (some Type 3 and subset fonts) are counted in a
  warning, not read. See [docs/pdf.md](docs/pdf.md).
- An `.xls` sheet whose cells are spread over more than 4 million positions
  is cut off by rows, with a warning.
- Damaged files are read as far as possible. What could not be read is
  listed as warnings in the YAML header.

## As a library

The binary is a thin layer over three crates. `doctopus-core` holds the
document model, format detection, embedded-document recursion and the
renderers. `doctopus-office` holds the Office readers and `doctopus-pdf`
the PDF reader.

```rust
use doctopus_core::render::{self, RenderOptions};
use doctopus_core::{Address, Engine, Registry};

let mut registry = Registry::new();
doctopus_office::register(&mut registry);
doctopus_pdf::register(&mut registry);
let engine = Engine::new(registry);
let doc = engine.open("report.docx")?;
let options = RenderOptions::new(Address::parse("report.docx"));
println!("{}", render::markdown(&doc, &options).text);
```

A new file type is a crate that implements the `Reader` trait and registers
itself. The engine, the renderers, embedded-document handling and the
command line need no changes. See [docs/adding-a-format.md](docs/adding-a-format.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Why "doctopus"

It is a document octopus: one tool with an arm in every document format,
reaching into the documents nested inside other documents.

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE)
or the [MIT license](LICENSE-MIT), at your option. Contributions are
accepted under the same terms. See [NOTICE](NOTICE) for credits.
