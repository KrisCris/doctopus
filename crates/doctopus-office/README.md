# doctopus-office

Microsoft Office readers for doctopus:

- Word: `.docx`, `.doc`
- Excel: `.xlsx`, `.xlsb`, `.xls`
- PowerPoint: `.pptx`, `.ppt`
- Visio: `.vsdx`, `.vsd`

Their macro and template variants are read too. Office Open XML and the
binary formats of Office 97-2003 are both covered.

```rust
let mut registry = doctopus_core::Registry::new();
doctopus_office::register(&mut registry);
```

Licensed under either of Apache-2.0 or MIT, at your option.
