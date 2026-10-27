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
- Embedded documents, images and attachments are read recursively. Each
  one can be addressed as `FILE#id/id`.
- `doctopus read`: Markdown, plain text or JSON output, paged, with
  selection by sheet, slide, page and cell range.
- `doctopus info`, `doctopus extract`, `doctopus formats` and
  `doctopus guide`.
- Self-contained release binaries for Linux, macOS and Windows on x86_64
  and arm64.
