//! Office Open XML (ECMA-376 / ISO/IEC 29500): the zip-based formats of
//! Office 2007 and later.

mod chart;
mod context;
pub(crate) mod docx;
mod drawing;
mod math;
mod package;
pub(crate) mod pptx;
mod props;
mod smartart;
pub(crate) mod vsdx;
pub(crate) mod xlsx;
mod xml;

use doctopus_core::{Confidence, Detection, FormatInfo, Probe};

use crate::formats::{DOCX, PPTX, VSDX, XLSB, XLSX};

/// Main-part content types (lower case) and the format they identify.
const MAIN_CONTENT_TYPES: &[(&str, &FormatInfo)] = &[
    ("wordprocessingml.document.main+xml", &DOCX),
    ("wordprocessingml.template.main+xml", &DOCX),
    ("ms-word.document.macroenabled.main+xml", &DOCX),
    ("ms-word.template.macroenabledtemplate.main+xml", &DOCX),
    ("spreadsheetml.sheet.main+xml", &XLSX),
    ("spreadsheetml.template.main+xml", &XLSX),
    ("ms-excel.sheet.macroenabled.main+xml", &XLSX),
    ("ms-excel.template.macroenabled.main+xml", &XLSX),
    ("ms-excel.addin.macroenabled.main+xml", &XLSX),
    ("ms-excel.sheet.binary.macroenabled.main", &XLSB),
    ("presentationml.presentation.main+xml", &PPTX),
    ("presentationml.slideshow.main+xml", &PPTX),
    ("presentationml.template.main+xml", &PPTX),
    ("ms-powerpoint.presentation.macroenabled.main+xml", &PPTX),
    ("ms-powerpoint.slideshow.macroenabled.main+xml", &PPTX),
    ("ms-powerpoint.template.macroenabled.main+xml", &PPTX),
    ("ms-powerpoint.addin.macroenabled.main+xml", &PPTX),
    ("ms-visio.drawing.main+xml", &VSDX),
    ("ms-visio.template.main+xml", &VSDX),
    ("ms-visio.stencil.main+xml", &VSDX),
    ("ms-visio.drawing.macroenabled.main+xml", &VSDX),
    ("ms-visio.template.macroenabled.main+xml", &VSDX),
    ("ms-visio.stencil.macroenabled.main+xml", &VSDX),
];

/// Main parts of packages whose content types are missing or damaged.
const MAIN_PARTS: &[(&str, &FormatInfo)] = &[
    ("word/document.xml", &DOCX),
    ("xl/workbook.xml", &XLSX),
    ("xl/workbook.bin", &XLSB),
    ("ppt/presentation.xml", &PPTX),
    ("visio/document.xml", &VSDX),
];

/// Recognise an Office Open XML package of the given format.
pub(crate) fn detect(probe: &Probe<'_>, format: &'static FormatInfo) -> Option<Detection> {
    if probe.is_cfb() && probe.cfb_has("EncryptedPackage") {
        // Encrypted: only the extension hints at what is inside. Without one,
        // the Word reader takes it and reports it as encrypted.
        let ext = probe.extension();
        if ext
            .as_deref()
            .is_some_and(|e| format.extensions.contains(&e))
        {
            return Some(Detection::new(format, Confidence::Medium));
        }
        let any_ooxml = ext
            .as_deref()
            .is_some_and(|e| MAIN_PARTS.iter().any(|(_, f)| f.extensions.contains(&e)));
        return (!any_ooxml && format.id == DOCX.id)
            .then(|| Detection::new(format, Confidence::Low));
    }
    if !probe.is_zip() {
        return None;
    }
    if let Some(types) = probe.zip_read("[Content_Types].xml") {
        let types = String::from_utf8_lossy(&types).to_ascii_lowercase();
        let found = MAIN_CONTENT_TYPES
            .iter()
            .find(|(needle, _)| types.contains(needle))
            .map(|(_, f)| *f);
        if let Some(found) = found {
            return (found.id == format.id).then(|| Detection::certain(format));
        }
    }
    MAIN_PARTS
        .iter()
        .find(|(part, f)| f.id == format.id && probe.zip_contains(part))
        .map(|_| Detection::new(format, Confidence::Medium))
}

/// `2021-03-04T10:22:33.123Z` → `2021-03-04 10:22`.
pub(crate) fn short_datetime(value: &str) -> String {
    let value = value.trim();
    match value.split_once('T') {
        Some((date, time)) if time.len() >= 5 => format!("{date} {}", &time[..5]),
        _ => value.to_string(),
    }
}
