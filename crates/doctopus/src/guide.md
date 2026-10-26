# doctopus guide

doctopus prints Office files as text: Word (.doc .docx), Excel (.xls .xlsx
.xlsb), PowerPoint (.ppt .pptx) and Visio (.vsd .vsdx), including macro and
template variants (`doctopus formats` lists them all). Whatever is embedded
in them is read too: a workbook in a slide, a document in that workbook,
images, attachments.

## Workflow

1. `doctopus info FILE` shows the format, properties, sheets, slides or
   pages, every embedded part and the length of the full output in
   characters and tokens. It ends with suggested next commands.
2. `doctopus read FILE` prints the content as Markdown. `doctopus FILE` does
   the same. A call prints at most 25000 characters. When there is more, a
   note on stderr gives the exact command for the next page
   (`--offset N`).
3. Read only what you need:
   - `--sheet 2`, `--slide 1,3-5`, `--page Summary`, `--sheet "Q*"`: numbers,
     ranges or names, where `*` matches anything. `--sheet`, `--slide`,
     `--page` and `--section` are the same option.
   - `--range A1:F50` limits spreadsheets to some cells. `B:D` selects
     columns and `5:40` selects rows.
   - `--formulas` shows spreadsheet formulas next to their values.
4. For a long output, `doctopus read FILE -o out.md` writes everything to a
   file. It prints the line where each sheet, slide or page (or each
   heading) starts, so you can read that file in chunks.

## Embedded content

Every image, embedded document and attachment has an id: `img1`, `emb2`,
`media1`. An address reaches inside a file with `#`, and nested ids are
joined with `/`:

    doctopus read "deck.pptx#emb1"           # embedded document emb1 alone
    doctopus info "deck.pptx#emb1"           # its overview and parts
    doctopus extract "deck.pptx#emb1/img2"   # save one image
    doctopus extract deck.pptx -o deck.parts # save every part
    doctopus read deck.pptx --extract deck.parts

`extract` lists each saved file with its address. Parts nested in an
embedded document go into a subdirectory named after that document's id.
With `read --extract DIR`, image and object references in the output point
at the saved files. You can open images saved this way (for example by
reading the file) to see them. Quote addresses that contain `#`, because
some shells treat it specially.

## Reading the output

- Markdown output starts with a YAML header: source, format, properties,
  counts, parts and warnings. `--no-front-matter` leaves the header out.
- Embedded documents appear inline between `<embedded id=... ref=...>` and
  `</embedded>`. Below `--embed-depth` levels they are referenced instead.
- Sheets become tables with column letters and row numbers. Sparse sheets
  are listed cell by cell (`- C7: value`).
- Diagrams list their shapes (`id [type] "text"`, with shape data and
  links) and then their connections (`from -> to`).
- Warnings say what could not be read, for example an unsupported object
  or a damaged part.
- `-f text` prints the same content as plain text. `-f json` prints the
  full document model, and JSON is never paged. `info --json`,
  `extract --json` and `formats --json` print JSON for programs.

## Errors

Errors go to stderr. Each one says what exists, for example the sheet
names or part ids, and suggests a command to try next. The exit codes are:

    0  success
    1  the file could not be read (missing, damaged)
    2  invalid arguments (unknown option, no such part, sheet or range)
    3  unsupported file type
    4  encrypted file

## Tips

- Set the page size with `--max-chars N` or `DOCTOPUS_MAX_CHARS`. Use
  `--max-chars 0` to print everything, for example when piping into grep.
- `doctopus info a.docx b.xlsx c.pptx` surveys several files in one call.
- Output is deterministic, so `--offset` pages always line up.
