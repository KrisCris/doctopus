# PDF

The PDF reader lives in `crates/doctopus-pdf`. This page explains how a PDF
maps onto the document model, which library it is built on and why, and
what it does not read.

## What is read

| PDF | Document model |
|---|---|
| Each page | A `Page` section, numbered from 1, so `--page 3` and `--page 2-4` work. A page label other than the page number (`iv`, `A-3`) is a `label` property. |
| Text | Paragraphs, headings and list items in reading order (below). Bold and italic come from the font. |
| Link annotations | Links on the text they cover: web addresses as they are, links within the document as `#page-N`, which the Markdown renderer points at the page's heading. |
| Pictures | Image parts, referenced where they are drawn. JPEG data is passed through unchanged; other pictures are converted to PNG. Pictures smaller than 6 points (rules, dots) are left out. |
| Bookmarks | A "Bookmarks" section: a nested list linking to `#page-N`. |
| Form fields | A "Form fields" section: a table of field names, types and values. |
| Attached files | Parts, read recursively. Files attached to a page (file attachment annotations) are shown on that page; the document's attachments (the `EmbeddedFiles` name tree) are listed after the pages. |
| Document information and XMP | Title, author, subject, keywords, application, producer, created and modified. The information dictionary wins; XMP fills in what it leaves out. The catalog's language and the PDF version are added. |
| Headers and footers | Text repeated at the top or bottom of at least four pages, and of at least half of the pages with text, is moved to a "Headers and footers" section, once. Page numbers count as repeated. |

PDF files embedded in Office files (OLE packages, `AcroExch.Document`
objects, package attachments) are parts of those files, so the engine reads
them with this reader. No Office reader needed a change.

### Reading order

A PDF page is a list of drawing operations: glyphs at positions, without
words, lines or paragraphs. The reader rebuilds them:

1. Glyphs are taken in the order the page draws them. Most writers draw
   text in reading order, column after column.
2. Glyphs on the same baseline, close enough together, form a line. A gap
   wider than 0.15 em is a space. Glyphs drawn twice at the same place
   (fake bold, shadows) are kept once.
3. Lines form a paragraph while they share a size, follow at the page's
   usual line spacing and overlap horizontally. A first-line indent, a list
   marker (`•`, `1.`, `a)`, `iv.`, `[3]`) or a larger gap starts a new
   one, unless the line is centered under the one above, as in a title.
   Lines are joined with a space, without one between CJK characters. A
   word hyphenated at the end of a line is joined again (`para-` and
   `graph`); a hyphen before a capital or a digit, or after a digit, is
   kept (`Stress-Engineered`, `4-piperidyl`).
4. A short paragraph (at most 4 lines and 200 characters, with letters in
   it) set at least 15% larger than the document's body text is a
   heading. Heading levels follow the sizes, largest first.
5. Text drawn in another direction (rotated a quarter turn, or upside down)
   is read along its own baseline.

Pages that draw text out of reading order, for example some magazine
layouts, may come out with paragraphs in the drawing order.

### Scanned pages

A page with no text but pictures, a scan or text set as a picture, is
reported in one warning with the ids of its pictures:

```text
2 pages have no text layer, only pictures (scans, or text set as pictures); extract the pictures to see them: page 1 (img1), page 2 (img2)
```

An agent that can view images can extract and look at them. A page with
no text but many vector shapes is reported as text drawn as shapes, and
glyphs that a font maps to no character are counted in a warning.

### Encryption

A file encrypted with an empty user password (the usual "owner password"
protection, which restricts printing or copying) is read. Any other
encrypted file fails with exit code 4, also when its cross-reference
table is damaged as well.

### Hostile input

The reader decodes nothing large without measuring it first:

- Before reading pages, every content, font and form stream is measured by
  decompressing it into a counter. One stream may expand to at most the
  part size limit (`ReadOptions::max_part_size`, 512 MiB), all of them
  together to four times that. Larger files are refused as damaged.
- A picture is decoded only when its dimensions (at most 64 million
  pixels) and its decoded data agree. JPEG 2000 and JPEG headers are
  checked against the picture's dimensions; JPEG and JPEG 2000 data that
  a writer wrapped in deflate or ASCII filters is measured, then unwrapped.
  A picture that fails a check is left out with a warning; the text of the
  page is still read.
- Pictures may take twice the part size limit together, a page at most
  1,000 pictures and 1,000,000 glyphs. Name trees, number trees, bookmarks
  and form fields are read to a depth of 32 and at most 100,000 entries.
- A page whose interpretation panics keeps what was read before the panic
  and gets a warning; the other pages are not affected.

## Choosing the library

A PDF library for doctopus has to be written in Rust (release binaries are
fully static, with no C or C++ dependency), permissively licensed, build
with Rust 1.88, and survive damaged files. That rules out pdfium, MuPDF
(which is also AGPL) and poppler. The candidates were:

| | lopdf 0.46 | pdf-extract 0.12 | pdf_oxide 0.3 | hayro 0.6 |
|---|---|---|---|---|
| What it gives | Objects; `extract_text` per page | Text per page, from lopdf | Text, layout and much more | Every glyph with its position, size and font; pictures |
| Rust version | 1.88 | not stated (lopdf 0.42) | 1.88 | 1.88 (0.7 and later need 1.92) |
| Panics in 400 damaged reads | 0 | 28 | 0 | 0 |
| ENERGY STAR guide (2,300 chars) | 0.67 ms | 1.52 ms | 2.93 ms | 0.91 ms |
| Tika web page print (1,050 chars) | no text | 1.43 ms | 3.03 ms | 0.69 ms |

The test set was the PDFs embedded in the Office test corpus and a few
PDFs that ship with macOS applications, 16 files. The corpus has since
grown by 54 PDF test files from Apache Tika and pdf.js (fonts, encodings,
encryption, attachments, scans, damaged files), which the reader's
snapshot and robustness tests use. Damaged copies were made
the way `tests/robustness.rs` makes them: flipped bytes, truncation, and
damaged decompressed streams. Times are release builds on an Apple M-series
machine, one read of the whole file.

- **lopdf** reads the file structure well but its text extraction misses
  whole files (the Quartz-generated page gave no text) and gives no
  positions, so there is nothing to rebuild paragraphs, headings or links
  from.
- **pdf-extract** gives plain text in drawing order, without positions,
  and panicked on 7% of the damaged files. The engine would contain the
  panic, but the whole document would be lost.
- **pdf_oxide** does its own layout analysis, which split lines of the test
  files into fragments. It is large (about 450,000 lines of Rust and 166
  dependencies) and slower on these files.
- **hayro** is a PDF interpreter: it runs the content streams with the
  graphics state, fonts (Type 1, TrueType, CFF, Type 3, CID-keyed), CMaps
  and `ToUnicode` maps, and reports each glyph through a `Device` trait.
  doctopus does the layout analysis itself on those glyphs, which is what
  the agent-first output needs (headings, links on the right words,
  pictures in place). It did not panic on any damaged file, its
  dependencies are Rust only, and its crates are Apache-2.0 OR MIT.

doctopus uses hayro 0.6, the last version that builds with Rust 1.88.
What it lacks, the reader adds:

- hayro decompresses without limits. `decode.rs` measures streams first
  (see above).
- Standard fonts that the file does not embed come with no font data, so
  bold and italic are read from the font's `/BaseFont` name instead.
- Type 3 fonts give no glyph widths to the interpreter; their glyphs are
  assumed half an em wide when spacing words.
- hayro reports some encrypted files with damaged cross-reference tables
  as damaged; the reader reports them as encrypted when the trailer has an
  `/Encrypt` entry.
- Text strings (titles, bookmarks, field values) that hold UTF-8 without
  the byte order mark are read as UTF-8, not as PDFDocEncoding.

### Size

hayro embeds the 14 standard fonts (Foxit fonts from PDFium, BSD-3-Clause)
and Adobe's predefined CMaps for Chinese, Japanese and Korean text
(BSD-3-Clause). Both are needed for correct text: the fonts give glyph
widths that files often leave out, and the CMaps map CJK character codes
to Unicode. On macOS arm64 the release binary grows from 2.9 MB to 6.5 MB
with the PDF reader. The fonts take 0.26 MB of that and the CMaps 0.46 MB;
the rest is the interpreter and its picture decoders (JPEG, JPEG 2000,
JBIG2, CCITT fax) and color management. The licenses of the fonts and
CMaps are in `THIRD-PARTY-LICENSES.txt` next to each binary.

## Not read

- Text in pictures. Scanned pages are reported, not recognized (see OCR
  below).
- Text drawn as vector shapes, which some design tools and printers
  produce.
- Tables are paragraphs: PDF has no table structure, and ruled lines are
  not used to rebuild one.
- Tagged PDF structure (the logical structure tree) is not used; reading
  order comes from the page.
- Annotations other than links and file attachments (comments, highlights,
  stamps), and form field appearances.
- Vertical CJK text is read glyph by glyph, not as vertical lines.
- Glyphs whose font does not say which characters they are: Type 3 fonts
  without a `ToUnicode` map (hayro 0.6 does not pass their character codes
  on), subset fonts that keep neither a character map nor glyph names, and
  ZapfDingbats symbols. They are counted in a warning.
- Files encrypted with a user password, and digital signatures. Streams
  that name their own crypt filter (`/Crypt`) are not decoded by hayro
  0.6; pictures and attached files encrypted that way are left out with a
  warning.
- Image masks: one-bit pictures painted in a color, as some logos and
  signatures are.

## OCR

Recognizing text in scanned pages needs a model. The only Rust-only OCR
library, [ocrs](https://github.com/robertknight/ocrs) with the rten
runtime, was evaluated and not added.

It was run (ocrs 0.13.1, rten 0.26, release build, Apple M-series) on
pages of the test corpus rendered at 200 dpi, with the reader's own text
as the truth, and on the corpus's scanned pictures. "Words found" is the
share of the page's words that OCR read correctly, in any order; the next
column is the share of the words OCR gave that are correct:

| Page | Words found | Found words that are right | Time |
|---|---|---|---|
| Tika web page print, one column | 97% | 99% | 0.7 s |
| TraceMonkey paper, two columns | 94% | 92% | 2.6 s |
| Journal paper with formulas | 88% | 96% | 2.7 s |
| Code of Federal Regulations, three columns of small print | 93% | 90% | 2.2 s |
| Scanned loan agreement, blurry 300 dpi JPEG 2000 (first 11 lines, checked by hand) | 65% | 69% | 4.9 s for the page |
| "Happy New Year 2003!" picture, "pdf.js" scan | exact; `pdf.is` | | 0.1 s |
| French accented letters, Arabic, Japanese | none | | |

A full page took 1.1 to 1.3 GB of memory at its peak, a small picture
0.2 GB.

- The models are licensed CC-BY-SA-4.0 (they are trained on the HierText
  dataset), which is not one of the permissive licenses doctopus ships.
- The two models are 12.2 MB, four times the size of the binary without
  them, and the runtime adds about 3.2 MB. A binary with them embedded
  would be about 18 MB.
- They read unaccented Latin letters only: the alphabet is ASCII, so `é`
  comes out as `?`, and other scripts give nothing.
- rten 0.26, which ocrs 0.13 needs, requires Rust 1.94 (rten 0.24 needs
  1.89); doctopus builds with 1.88.
- The detection model on the model hub (`text-detection-ssfbcj81.rten`)
  needs a mask threshold of 0.5, where ocrs 0.13 uses 0.2: with ocrs's
  own threshold it finds almost nothing usable. Using it means calling
  ocrs's lower-level functions or using the release models instead.
- The detection model takes a fixed 600 x 800 input and ocrs scales a
  whole page down to it, so small print is found less reliably. Cutting
  the page into tiles found a few more words but read the overlaps twice.

The options are to embed the models anyway, to load them from a directory
given by the user (`--ocr-models DIR` or an environment variable, so the
binary never downloads anything), or to leave recognition to agents that
can view the extracted page pictures. doctopus does the last for now.
