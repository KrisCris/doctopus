# doctopus-archive

Zip archive reader for doctopus. Every file in an archive becomes a part
named after its path in the archive, so the engine reads each one with its
own reader: a Word document in a zip, or a zip in a zip, is read like any
other embedded document.

Office Open XML files are zip archives too, and their readers win. Other
zip-based formats (OpenDocument, EPUB, Java archives) are left alone: the
reader only takes data named `.zip`, or not named at all.

```rust
let mut registry = doctopus_core::Registry::new();
doctopus_archive::register(&mut registry);
```

Archives are read within the engine's limits: entries larger than
`max_part_size` are refused, reading stops once the files unpack to more
than that in total, and at most 10,000 files are read.

Licensed under either of Apache-2.0 or MIT, at your option.
