//! The binary formats of Office 97-2003, stored in OLE compound files:
//! Word (`.doc`), Excel (`.xls`), PowerPoint (`.ppt`) and Visio (`.vsd`).

mod context;
pub(crate) mod doc;
pub(crate) mod officeart;
pub(crate) mod ppt;
pub(crate) mod vsd;
pub(crate) mod xls;

use doctopus_core::{Detection, FormatInfo, Probe};

use crate::formats::{DOC, PPT, VSD, XLS};

/// The root streams that identify each format.
const MAIN_STREAMS: &[(&str, &FormatInfo)] = &[
    ("WordDocument", &DOC),
    ("Workbook", &XLS),
    ("Book", &XLS),
    ("PowerPoint Document", &PPT),
    // PowerPoint 95, claimed only to say that it is not supported.
    ("PP40", &PPT),
    ("VisioDocument", &VSD),
];

/// Detect a 97-2003 file by its main stream.
pub(crate) fn detect(probe: &Probe<'_>, format: &'static FormatInfo) -> Option<Detection> {
    if !probe.is_cfb() {
        return None;
    }
    MAIN_STREAMS
        .iter()
        .find(|(stream, f)| f.id == format.id && probe.cfb_has(stream))
        .map(|_| Detection::certain(format))
}
