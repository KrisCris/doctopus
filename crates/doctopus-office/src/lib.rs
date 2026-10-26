//! Microsoft Office readers for [doctopus](https://crates.io/crates/doctopus):
//! Word, Excel, PowerPoint and Visio, in both the Office Open XML formats
//! (`.docx`, `.xlsx`, `.pptx`, `.vsdx` and their macro/template variants)
//! and the binary formats of Office 97-2003 (`.doc`, `.xls`, `.ppt`,
//! `.vsd`).
//!
//! ```
//! let mut registry = doctopus_core::Registry::new();
//! doctopus_office::register(&mut registry);
//! assert!(registry.formats().iter().any(|f| f.id == "pptx"));
//! ```

mod binary;
mod blocks;
mod dates;
mod diagram;
pub mod formats;
mod legacy;
mod numbering;
mod ole;
mod ooxml;
mod sheet;
mod word;

use doctopus_core::{Detection, Document, FormatInfo, Input, Probe, Reader, Registry, Result};

/// Register every reader of this crate.
pub fn register(registry: &mut Registry) {
    registry
        .register(DocxReader)
        .register(DocReader)
        .register(XlsxReader)
        .register(XlsbReader)
        .register(XlsReader)
        .register(PptxReader)
        .register(PptReader)
        .register(VsdxReader)
        .register(VsdReader);
}

/// Define a reader for one format.
macro_rules! office_reader {
    ($(#[$meta:meta])* $name:ident, $format:path, $detect:path, $read:path) => {
        $(#[$meta])*
        #[derive(Debug, Default, Clone, Copy)]
        pub struct $name;

        impl Reader for $name {
            fn formats(&self) -> &[&'static FormatInfo] {
                static FORMATS: [&FormatInfo; 1] = [&$format];
                &FORMATS
            }

            fn detect(&self, probe: &Probe<'_>) -> Option<Detection> {
                $detect(probe, &$format)
            }

            fn read(&self, input: &Input<'_>) -> Result<Document> {
                $read(input)
            }
        }
    };
}

office_reader!(
    /// Word documents (`.docx`, `.docm`, `.dotx`, `.dotm`).
    DocxReader,
    formats::DOCX,
    ooxml::detect,
    ooxml::docx::read
);

office_reader!(
    /// Excel workbooks (`.xlsx`, `.xlsm`, `.xltx`, `.xltm`, `.xlam`).
    XlsxReader,
    formats::XLSX,
    ooxml::detect,
    ooxml::xlsx::read
);

office_reader!(
    /// Excel binary workbooks (`.xlsb`): cell values and formulas.
    XlsbReader,
    formats::XLSB,
    ooxml::detect,
    ooxml::xlsx::read_binary
);

office_reader!(
    /// PowerPoint presentations (`.pptx`, `.pptm`, `.ppsx`, `.potx`, ...).
    PptxReader,
    formats::PPTX,
    ooxml::detect,
    ooxml::pptx::read
);

office_reader!(
    /// Visio drawings, stencils and templates (`.vsdx`, `.vsdm`, `.vssx`, `.vstx`, ...).
    VsdxReader,
    formats::VSDX,
    ooxml::detect,
    ooxml::vsdx::read
);

office_reader!(
    /// Word 97-2003 documents (`.doc`, `.dot`).
    DocReader,
    formats::DOC,
    legacy::detect,
    legacy::doc::read
);

office_reader!(
    /// Excel 97-2003 workbooks (`.xls`, `.xlt`, `.xla`).
    XlsReader,
    formats::XLS,
    legacy::detect,
    legacy::xls::read
);

office_reader!(
    /// Visio 5-2010 drawings, stencils and templates (`.vsd`, `.vss`, `.vst`).
    VsdReader,
    formats::VSD,
    legacy::detect,
    legacy::vsd::read
);

office_reader!(
    /// PowerPoint 97-2003 presentations (`.ppt`, `.pps`, `.pot`).
    PptReader,
    formats::PPT,
    legacy::detect,
    legacy::ppt::read
);
