# doctopus

**Read Word, Excel, PowerPoint, Visio and PDF files, and everything
embedded in them, as Markdown that coding agents can work with, or convert
them to Markdown files once and read those.**

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
- **A clean conversion when you want one.** `doctopus convert` writes a
  file and every document inside it, to any depth, as Markdown files that
  link to each other and to the saved pictures. Afterwards they are read
  without doctopus.
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
  - doctopus extract deck.pptx  # save images and embedded files next to the file
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

**Prebuilt binaries.** Every release has an archive for each of these
targets. Download the one for your platform from the
[releases page](https://github.com/KrisCris/doctopus/releases), unpack it,
and put `doctopus` (or `doctopus.exe`) on your `PATH`.

| Platform | x86_64 | arm64 |
|---|---|---|
| Linux, any distribution (static, musl) | `x86_64-unknown-linux-musl` | `aarch64-unknown-linux-musl` |
| Linux with glibc 2.35 or newer | `x86_64-unknown-linux-gnu` | `aarch64-unknown-linux-gnu` |
| macOS | `x86_64-apple-darwin` | `aarch64-apple-darwin` |
| Windows | `x86_64-pc-windows-msvc` | `aarch64-pc-windows-msvc` |

Archives are named `doctopus-VERSION-TARGET.tar.gz`, or `.zip` on
Windows. On Linux, the musl binaries are statically linked and run on any
distribution, including Alpine and older ones; if unsure, pick those. The
gnu binaries link the system's glibc dynamically, like most Linux programs,
and need glibc 2.35 or newer (Ubuntu 22.04, Debian 12, RHEL 10 and later).
The Windows binaries do not need the Visual C++ runtime.

Every CI run, on `main` and on pull requests, builds the same eight
archives. To try a change before it is released, download one from the
run's summary page under **Artifacts**; they are kept for 14 days.

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
content as Markdown, `doctopus convert FILE -o DIR` to write it all out as
Markdown files. `doctopus guide` explains the rest.
```

## Usage

| Command | What it does |
|---|---|
| `doctopus info FILE...` | Format, properties, sheets, slides or pages, embedded parts, output size, and suggested next commands |
| `doctopus read FILE` | The content as Markdown, a page at a time (`doctopus FILE` is short for this) |
| `doctopus extract FILE [-o DIR]` | Save every image, embedded document and attachment, nested ones included (to `NAME.parts/` next to the file by default) |
| `doctopus convert FILE... [-o DIR]` | Write each file, and every document inside it, as Markdown files with their pictures and attachments (next to the file by default) |
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
paging off. Links to saved parts in a file written with `-o` are relative
to that file.

### Converting to Markdown files

```sh
doctopus convert docs/report.docx              # writes docs/report.md
doctopus convert report.docx -o converted
doctopus convert *.docx *.xlsx -o converted   # several files at once
```

A document becomes `NAME.md`, and its pictures and attached files go to
`NAME.parts/`, in the directory given with `-o` or else next to the
document. The command prints that directory as an absolute path and the
file to read first. A `NAME.md` already there is replaced only when its
header says it is an earlier conversion of the same file. Anything else is
left alone, and so is a `NAME.parts/` without such a `NAME.md`; the
conversion is then written as `NAME-2.md`. Every embedded document is
converted too, however deep it is nested: it gets a directory named after
its id inside its parent's parts, with its own Markdown file and parts.

```text
converted/report.md
converted/report.parts/image1.png
converted/report.parts/emb1/Budget.md         # a workbook in the document
converted/report.parts/emb1/emb2/Notes.md     # a document in that workbook
```

Formatting carries over where Markdown has a form for it. A Word Heading 1
becomes `#`, Heading 2 `##`, and so on. Bold, italic, strikethrough,
superscript and subscript are kept, and so are nested and numbered lists,
line breaks, tables, footnotes and links. Text in a fixed-width font
becomes `code`, paragraphs in a code style become code blocks, and
quotation styles become `>` quotes. Table-of-contents entries link to their
headings. Underline, colors and fonts have no Markdown form and are
dropped.

Embedded documents are linked where they appear instead of being inlined,
and every link is relative, so the directory can be moved as a whole. The
Markdown never refers to doctopus commands. Visio pages keep their shape
list and connection list, and also get a [Mermaid](https://mermaid.js.org/)
flowchart of the connections (up to 200), which renders on GitHub and in
most Markdown viewers. Files doctopus cannot read, such as audio or
programs, are saved as they are and linked. The command prints what it
wrote; `--json` prints it as JSON.

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
| Word | `.docx` `.docm` `.dotx` `.dotm` `.doc` `.dot` | Text with headings at their levels, lists, tables, links, quotations, code and text boxes; equations as LaTeX; footnotes, endnotes, comments, headers and footers; images, charts, SmartArt, embedded objects |
| Excel | `.xlsx` `.xlsm` `.xltx` `.xltm` `.xlam` `.xlsb` `.xls` `.xlt` `.xla` | Every sheet as a table with column letters and row numbers (sparse sheets cell by cell); formulas, merged cells, hyperlinks, notes and threaded comments, headers and footers, charts, pictures, text boxes, embedded objects |
| PowerPoint | `.pptx` `.pptm` `.ppsx` `.ppsm` `.potx` `.potm` `.ppt` `.pps` `.pot` | Slides in order with titles, text, tables, pictures, charts, SmartArt and embedded objects; speaker notes and comments |
| Visio | `.vsdx` `.vsdm` `.vssx` `.vssm` `.vstx` `.vstm` `.vsd` `.vss` `.vst` | Pages of shapes with their text, shape data and hyperlinks; groups; the connections between shapes; pictures and embedded objects |
| PDF | `.pdf` | Pages with their text in reading order as paragraphs, headings and lists; links; pictures where they appear; bookmarks, page labels, form field values and attached files. Pages that are only pictures (scans) are reported |
| Zip archive | `.zip` | Every file in the archive, each read by its own reader: a workbook in a zip, a zip in a zip |
| Plain text | `.txt` `.text` `.log` `.csv` `.tsv` `.md` `.markdown` `.json` `.yaml` `.yml` `.ini` | The text exactly as written, as a code block. Mostly useful for text files embedded in other documents |

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
- Embedded HTML, RTF and Outlook messages, and OLE objects of unknown
  type, are saved as files, not read. Pictures are saved in their own
  format; EMF and WMF drawings are not converted to PNG.
- Damaged files are read as far as possible. What could not be read is
  listed as warnings in the YAML header.

## As a library

The binary is a thin layer over a few crates. `doctopus-core` holds the
document model, format detection, embedded-document recursion and the
renderers. `doctopus-office` holds the Office readers, `doctopus-pdf` the
PDF reader, `doctopus-archive` the zip reader and `doctopus-text` the
plain-text reader.

```rust
use doctopus_core::render::{self, RenderOptions};
use doctopus_core::{Address, Engine, Registry};

let mut registry = Registry::new();
doctopus_office::register(&mut registry);
doctopus_pdf::register(&mut registry);
doctopus_archive::register(&mut registry);
doctopus_text::register(&mut registry);
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
