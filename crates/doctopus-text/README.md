# doctopus-text

Plain-text reader for doctopus. Text files (`.txt`, `.log`, `.csv`, `.md`,
`.json` and a few more) become one code block holding the text exactly as
written, so a text file attached to a document, or packed in an archive, is
read along with it. Files are recognised by their extension, and only when
they hold no NUL bytes.

UTF-8 and UTF-16 with a byte order mark are decoded as such; other text
that is not valid UTF-8 is read as Windows-1252, with a warning.

```rust
let mut registry = doctopus_core::Registry::new();
doctopus_text::register(&mut registry);
```

Licensed under either of Apache-2.0 or MIT, at your option.
