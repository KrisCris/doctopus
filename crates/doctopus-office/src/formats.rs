//! Descriptions of the formats this crate reads.

use doctopus_core::{DocumentKind, FormatInfo};

/// Word documents, templates and macro-enabled variants (Office Open XML).
pub static DOCX: FormatInfo = FormatInfo {
    id: "docx",
    name: "Word document (Office Open XML)",
    kind: DocumentKind::Text,
    extensions: &["docx", "docm", "dotx", "dotm"],
    mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
};

/// Excel workbooks, templates, add-ins and macro-enabled variants (Office
/// Open XML).
pub static XLSX: FormatInfo = FormatInfo {
    id: "xlsx",
    name: "Excel workbook (Office Open XML)",
    kind: DocumentKind::Spreadsheet,
    extensions: &["xlsx", "xlsm", "xltx", "xltm", "xlam"],
    mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
};

/// Excel binary workbooks.
pub static XLSB: FormatInfo = FormatInfo {
    id: "xlsb",
    name: "Excel binary workbook",
    kind: DocumentKind::Spreadsheet,
    extensions: &["xlsb"],
    mime: "application/vnd.ms-excel.sheet.binary.macroenabled.12",
};

/// PowerPoint presentations, slide shows, templates and macro-enabled
/// variants (Office Open XML).
pub static PPTX: FormatInfo = FormatInfo {
    id: "pptx",
    name: "PowerPoint presentation (Office Open XML)",
    kind: DocumentKind::Presentation,
    extensions: &["pptx", "pptm", "ppsx", "ppsm", "potx", "potm"],
    mime: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
};

/// Visio drawings, stencils and templates (Office Open XML, Visio 2013+).
pub static VSDX: FormatInfo = FormatInfo {
    id: "vsdx",
    name: "Visio drawing (Office Open XML)",
    kind: DocumentKind::Diagram,
    extensions: &["vsdx", "vsdm", "vssx", "vssm", "vstx", "vstm"],
    mime: "application/vnd.ms-visio.drawing",
};

/// Word 97-2003 documents and templates.
pub static DOC: FormatInfo = FormatInfo {
    id: "doc",
    name: "Word 97-2003 document",
    kind: DocumentKind::Text,
    extensions: &["doc", "dot"],
    mime: "application/msword",
};

/// Excel 97-2003 workbooks (BIFF8; older BIFF versions are read on a
/// best-effort basis).
pub static XLS: FormatInfo = FormatInfo {
    id: "xls",
    name: "Excel 97-2003 workbook",
    kind: DocumentKind::Spreadsheet,
    extensions: &["xls", "xlt", "xla"],
    mime: "application/vnd.ms-excel",
};

/// PowerPoint 97-2003 presentations.
pub static PPT: FormatInfo = FormatInfo {
    id: "ppt",
    name: "PowerPoint 97-2003 presentation",
    kind: DocumentKind::Presentation,
    extensions: &["ppt", "pps", "pot"],
    mime: "application/vnd.ms-powerpoint",
};

/// Visio 2003-2010 binary drawings (and Visio 5/2000/2002 on a best-effort
/// basis).
pub static VSD: FormatInfo = FormatInfo {
    id: "vsd",
    name: "Visio 2003-2010 drawing",
    kind: DocumentKind::Diagram,
    extensions: &["vsd", "vss", "vst"],
    mime: "application/vnd.visio",
};
