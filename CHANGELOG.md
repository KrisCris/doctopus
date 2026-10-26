# Changelog

All notable changes to doctopus are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Readers for Word (`.docx`, `.doc`), Excel (`.xlsx`, `.xlsb`, `.xls`),
  PowerPoint (`.pptx`, `.ppt`) and Visio (`.vsdx`, `.vsd`), with their
  macro and template variants.
- Embedded documents, images and attachments are read recursively. Each
  one can be addressed as `FILE#id/id`.
- `doctopus read`: Markdown, plain text or JSON output, paged, with
  selection by sheet, slide, page and cell range.
- `doctopus info`, `doctopus extract`, `doctopus formats` and
  `doctopus guide`.
- Self-contained release binaries for Linux, macOS and Windows on x86_64
  and arm64.
