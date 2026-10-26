# doctopus-core

The format-independent core of doctopus:

- the document model: sections, blocks, parts and metadata;
- the `Reader` trait that file formats implement, and the `Registry`
  that collects readers;
- the `Engine`, which detects formats and parses embedded documents
  recursively;
- size-limited access to zip archives and OLE compound files;
- renderers for Markdown, plain text, JSON and summaries.

Readers for Office files are in `doctopus-office`. To add a file type, see
`docs/adding-a-format.md` in the doctopus repository.

Licensed under either of Apache-2.0 or MIT, at your option.
