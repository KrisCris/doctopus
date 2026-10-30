# Changelog

All notable changes to doctopus are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Readers for Word (`.docx`, `.doc`), Excel (`.xlsx`, `.xlsb`, `.xls`),
  PowerPoint (`.pptx`, `.ppt`) and Visio (`.vsdx`, `.vsd`), with their
  macro and template variants.
- A PDF reader (`.pdf`): text in reading order with headings, lists and
  links; pictures; bookmarks, page labels, form field values, document
  properties and attached files. Scanned pages are reported. PDF files
  embedded in Office documents are read too.
- Readers for zip archives, whose files are read by their own readers,
  and for plain-text files, kept exactly as written.
- Embedded documents, images and attachments are read recursively, 32
  levels deep by default (`--max-depth`). Each one can be addressed as
  `FILE#id/id`. Documents nested deeper are listed as not parsed.
- `doctopus read`: Markdown, plain text or JSON output, paged, with
  selection by sheet, slide, page and cell range. Markdown keeps heading
  levels, emphasis, nested lists, line breaks, quotations, code (Word code
  styles and fixed-width text) and links from a table of contents to its
  headings.
- `doctopus convert`: a file and every document inside it, to any depth,
  as Markdown files linked to each other and to the saved pictures and
  attachments, to be read without doctopus. Visio pages also get a
  Mermaid flowchart of their connections. Without `-o`, files are written
  next to the document, and a Markdown file of the user's own is never
  overwritten.
- `convert`, `extract` and `read -o` report where they wrote as absolute
  paths.
- `doctopus info`, `doctopus extract`, `doctopus formats` and
  `doctopus guide`.
- Release binaries for Linux, macOS and Windows on x86_64 and arm64. Linux
  has two builds: statically linked with musl, for any distribution, and
  linked against glibc 2.35 or newer. Every CI run builds them too and
  keeps them as downloadable artifacts for 14 days.
