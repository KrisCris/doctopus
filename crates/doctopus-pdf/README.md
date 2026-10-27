# doctopus-pdf

PDF reader for doctopus. Every page becomes a page section with its text in
reading order: paragraphs, headings, list items, links, and the pictures
where they appear. Bookmarks, page labels, form field values, document
properties (the information dictionary and XMP) and attached files are
read too. Attached files are parts, so an attached Word document is read
by the Word reader, and a PDF embedded in a Word document is read by this
one.

```rust
let mut registry = doctopus_core::Registry::new();
doctopus_pdf::register(&mut registry);
```

The reader is built on [hayro](https://github.com/LaurenzV/hayro), a PDF
interpreter written in Rust, and has no C dependencies.
[docs/pdf.md](../../docs/pdf.md) explains how PDF maps onto the document
model, why hayro was chosen, and what is not read.

Licensed under either of Apache-2.0 or MIT, at your option.
