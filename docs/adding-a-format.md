# Adding a file format

doctopus knows file formats only through readers. A reader recognizes a
format and turns its bytes into the common document model. Everything
else comes from `doctopus-core`: detection across readers, parsing of
embedded content, rendering to Markdown, text and JSON, addresses, paging
and extraction. Adding a format therefore means writing one crate. The
engine, the renderers and most of the command line stay as they are.

## How a file is read

```text
bytes ──> Probe ──> Reader::detect, for every registered reader
                          │  the most confident one wins
                          v
                    Reader::read ──> Document
                                        │  sections of blocks, metadata,
                                        │  parts (images, embedded files)
                                        v
                    Engine: each part that some reader recognizes is read
                    the same way and attached as part.document, to any depth
                                        │
                                        v
                    render::markdown / text / json / info
```

A reader never parses embedded content itself. It attaches the payload as
a `Part`, and the engine hands that part to whichever reader recognizes
it. A PDF reader added later would make PDFs embedded in Word documents
readable without changing the Word reader.

## A worked example: CSV

The crate below reads `.csv` files as one sheet. It is deliberately
minimal: it has no quoting rules.

```rust
//! Comma-separated values for doctopus.

use doctopus_core::{
    Block, Confidence, Detection, Document, DocumentKind, Error, FormatInfo, Grid, GridCell,
    GridRow, Input, Probe, Reader, Registry, Result, Section, SectionKind, Value,
};

/// CSV files.
pub static CSV: FormatInfo = FormatInfo {
    id: "csv",
    name: "Comma-separated values",
    kind: DocumentKind::Spreadsheet,
    extensions: &["csv"],
    mime: "text/csv",
};

/// Reads `.csv` files as a sheet.
#[derive(Debug, Default, Clone, Copy)]
pub struct CsvReader;

impl Reader for CsvReader {
    fn formats(&self) -> &[&'static FormatInfo] {
        static FORMATS: [&FormatInfo; 1] = [&CSV];
        &FORMATS
    }

    fn detect(&self, probe: &Probe<'_>) -> Option<Detection> {
        // CSV has no signature: go by the extension, and only for text.
        let text = !probe.data().iter().take(4096).any(|&b| b == 0);
        (text && probe.extension().as_deref() == Some("csv"))
            .then(|| Detection::new(&CSV, Confidence::Low))
    }

    fn read(&self, input: &Input<'_>) -> Result<Document> {
        let text = std::str::from_utf8(input.data)
            .map_err(|_| Error::malformed(input.format.name, "the file is not UTF-8 text"))?;
        let mut grid = Grid {
            dimension: None,
            rows: Vec::new(),
            merged: Vec::new(),
        };
        for (row, line) in text.lines().enumerate() {
            let cells: Vec<GridCell> = line
                .split(',')
                .enumerate()
                .filter(|(_, field)| !field.is_empty())
                .map(|(col, field)| {
                    let value = match field.parse::<f64>() {
                        Ok(n) => Value::Number(n),
                        Err(_) => Value::Text(field.to_string()),
                    };
                    GridCell::new(col as u32, value)
                })
                .collect();
            if !cells.is_empty() {
                grid.rows.push(GridRow {
                    row: row as u32,
                    cells,
                });
            }
        }
        let mut doc = Document::new(input.format);
        let mut sheet = Section::new(SectionKind::Sheet).with_number(1);
        sheet.blocks.push(Block::Grid(grid));
        doc.sections.push(sheet);
        Ok(doc)
    }
}

/// Register the readers of this crate.
pub fn register(registry: &mut Registry) {
    registry.register(CsvReader);
}
```

To make the command line read the new format, add the crate to
`crates/doctopus/Cargo.toml` and register it in `engine()` in
`crates/doctopus/src/load.rs`:

```rust
doctopus_office::register(&mut registry);
doctopus_csv::register(&mut registry);
```

From then on `doctopus read budget.csv` works. So do `info`, `extract`,
`--sheet`, `--range` and paging, and so does a CSV file embedded in a Word
document.

## Writing a reader

### Detection

`detect` runs for every input and for every embedded part, so it must be
cheap and must never panic. `Probe` computes zip entry names and compound
file entries once and shares them between readers. Use it instead of
opening containers yourself.

Report how sure you are:

- `Confidence::High`: a conclusive signature, such as a package content
  type or a stream that only this format has.
- `Confidence::Medium`: the container matches but the signature is not
  conclusive.
- `Confidence::Low`: only the file name says so.

When two readers claim an input with the same confidence, the one
registered first wins.

### The document model

- **Sections.** Use `Sheet`, `Slide` and `Page` for the units users select
  with `--sheet`, `--slide` and `--page`, and give them numbers and names
  as the application shows them. Use `Body` for flowing text. Headers and
  footers, footnotes, endnotes and comments get sections of their own.
- **Blocks.** Headings, paragraphs, list items, tables (rich cells),
  grids (typed spreadsheet cells), images, embeds, charts, diagrams, math
  and notes. `Group` labels a container such as speaker notes or a text
  box. Keep the order a person reads in.
- **Parts.** Every image, embedded document or attached file is a `Part`
  added with `Document::add_part`. Reference it from the content with
  `Block::Image` or `Block::Embed`. Set `name`, `content_type`,
  `location` (such as `slide 3` or `sheet "Data" cell B4`) and `origin`
  (the path inside the container, so a part used twice is stored once).
  Parts that no block references are still listed under "Other embedded
  content".
- **Metadata.** Title, author, dates and custom properties go into
  `Document::metadata`.

### Damaged and hostile input

Readers parse untrusted files. The rules:

- Return what you could read. Report the rest with `Document::warn`. Fail
  with `Error::Malformed` only when nothing useful is left, and with
  `Error::Encrypted` for protected files.
- Never panic. The engine catches panics and turns them into errors, but a
  panic loses the whole document. Use checked arithmetic and `get` instead
  of indexing on anything a file controls.
- Never trust sizes or counts read from the file. Bound allocations and
  loops, follow `input.options.max_part_size`, and read containers through
  `doctopus_core::container`, which enforces it.
- Keep output deterministic. Do not let `HashMap` iteration order reach
  the output.
- Do no I/O beyond the bytes you are given.

## Testing

- **Unit tests** next to the code, built on small inputs constructed in
  the test. See the record builders in `crates/doctopus-office/src/legacy/xls.rs`.
- **Corpus snapshots.** Add real sample files to `testdata/corpus.tsv`.
  The source's base URL, pinned to a commit, goes in
  `scripts/fetch-corpus.sh`. Run `scripts/fetch-corpus.sh --pin` to
  download the files and record their hashes. The snapshot test
  (`crates/doctopus-office/tests/corpus.rs`) renders every corpus file.
  Review the new snapshots with `cargo insta review` before committing
  them.
- **Robustness.** `crates/doctopus-office/tests/robustness.rs` damages
  every corpus file in many ways and fails if a reader panics or is slow.

Both tests use the readers of `doctopus-office`. A reader in a new crate
gets its own copy of them, registering its own readers.

## Checklist

- [ ] Readers for the format, registered in the crate's `register`
- [ ] Registered in `crates/doctopus/src/load.rs`
- [ ] Unit tests, corpus files and reviewed snapshots
- [ ] `cargo test --workspace`, `cargo clippy --workspace --all-targets`
  and the robustness test pass
- [ ] The format table in `README.md` and the list in `doctopus guide`
  (`crates/doctopus/src/guide.md`) are updated
- [ ] Sources of format knowledge are credited in `NOTICE`
