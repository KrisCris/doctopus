//! Content sniffing: recognise common file types from their first bytes.
//!
//! Used to label embedded payloads that no reader handles (so a caller learns
//! that `emb3` is a PDF it can process elsewhere) and to explain
//! "unsupported format" errors.

/// A recognised file type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sniffed {
    /// Media type.
    pub mime: &'static str,
    /// Usual extension, without the dot.
    pub extension: &'static str,
    /// Human-readable name.
    pub description: &'static str,
}

const fn sniffed(
    mime: &'static str,
    extension: &'static str,
    description: &'static str,
) -> Sniffed {
    Sniffed {
        mime,
        extension,
        description,
    }
}

/// Identify `data` from magic numbers. Container formats (zip, OLE compound
/// files) are reported generically; readers refine them.
pub fn sniff(data: &[u8]) -> Option<Sniffed> {
    let starts = |magic: &[u8]| data.starts_with(magic);
    let at = |offset: usize, magic: &[u8]| data.get(offset..offset + magic.len()) == Some(magic);

    let found = if starts(b"\x89PNG\r\n\x1a\n") {
        sniffed("image/png", "png", "PNG image")
    } else if starts(&[0xFF, 0xD8, 0xFF]) {
        sniffed("image/jpeg", "jpg", "JPEG image")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        sniffed("image/gif", "gif", "GIF image")
    } else if starts(b"BM") && data.len() > 14 && at(6, &[0, 0, 0, 0]) {
        sniffed("image/bmp", "bmp", "BMP image")
    } else if starts(b"II*\0") || starts(b"MM\0*") {
        sniffed("image/tiff", "tif", "TIFF image")
    } else if starts(b"RIFF") && at(8, b"WEBP") {
        sniffed("image/webp", "webp", "WebP image")
    } else if starts(&[0, 0, 1, 0]) && data.len() > 6 && data[4] > 0 {
        sniffed("image/x-icon", "ico", "icon")
    } else if starts(&[1, 0, 0, 0]) && at(40, b" EMF") {
        sniffed("image/emf", "emf", "Enhanced Metafile (EMF) image")
    } else if starts(&[0xD7, 0xCD, 0xC6, 0x9A])
        || starts(&[1, 0, 9, 0, 0, 3])
        || starts(&[2, 0, 9, 0, 0, 3])
    {
        sniffed("image/wmf", "wmf", "Windows Metafile (WMF) image")
    } else if starts(b"%PDF-") {
        sniffed("application/pdf", "pdf", "PDF document")
    } else if starts(b"{\\rtf") {
        sniffed("application/rtf", "rtf", "RTF document")
    } else if starts(&[0x09, 0x00, 0x04, 0x00])
        || starts(&[0x09, 0x02, 0x06, 0x00])
        || starts(&[0x09, 0x04, 0x06, 0x00])
    {
        // BIFF2-4 files are a bare record stream, not a compound file.
        sniffed("application/vnd.ms-excel", "xls", "Excel 2.x-4.0 worksheet")
    } else if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
        sniffed("application/zip", "zip", "zip archive")
    } else if starts(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        sniffed("application/x-ole-storage", "ole", "OLE compound file")
    } else if starts(&[0x1F, 0x8B]) {
        sniffed("application/gzip", "gz", "gzip archive")
    } else if starts(b"7z\xBC\xAF\x27\x1C") {
        sniffed("application/x-7z-compressed", "7z", "7-Zip archive")
    } else if starts(b"Rar!\x1A\x07") {
        sniffed("application/vnd.rar", "rar", "RAR archive")
    } else if starts(b"ID3")
        || starts(&[0xFF, 0xFB])
        || starts(&[0xFF, 0xF3])
        || starts(&[0xFF, 0xF2])
    {
        sniffed("audio/mpeg", "mp3", "MP3 audio")
    } else if starts(b"RIFF") && at(8, b"WAVE") {
        sniffed("audio/wav", "wav", "WAV audio")
    } else if starts(b"RIFF") && at(8, b"AVI ") {
        sniffed("video/x-msvideo", "avi", "AVI video")
    } else if starts(b"OggS") {
        sniffed("audio/ogg", "ogg", "Ogg audio")
    } else if starts(b"fLaC") {
        sniffed("audio/flac", "flac", "FLAC audio")
    } else if at(4, b"ftypqt") {
        sniffed("video/quicktime", "mov", "QuickTime video")
    } else if at(4, b"ftypM4A") {
        sniffed("audio/mp4", "m4a", "MPEG-4 audio")
    } else if at(4, b"ftyp") {
        sniffed("video/mp4", "mp4", "MPEG-4 video")
    } else if starts(&[0x1A, 0x45, 0xDF, 0xA3]) {
        sniffed("video/webm", "webm", "WebM/Matroska video")
    } else if starts(b"MZ") {
        sniffed(
            "application/vnd.microsoft.portable-executable",
            "exe",
            "Windows executable",
        )
    } else if starts(b"\x7FELF") {
        sniffed("application/x-elf", "elf", "ELF executable")
    } else {
        return sniff_text(data);
    };
    Some(found)
}

fn sniff_text(data: &[u8]) -> Option<Sniffed> {
    let head = &data[..data.len().min(4096)];
    let head = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    let text = match std::str::from_utf8(head) {
        Ok(t) => t,
        // A multi-byte character may be cut at the 4 KiB boundary.
        Err(e) if e.error_len().is_none() && e.valid_up_to() > 0 => {
            std::str::from_utf8(&head[..e.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    if text.is_empty() || text.chars().any(|c| c.is_control() && !c.is_whitespace()) {
        return None;
    }
    let lower = text.trim_start().to_ascii_lowercase();
    Some(
        if lower.starts_with("<svg") || (lower.starts_with("<?xml") && lower.contains("<svg")) {
            sniffed("image/svg+xml", "svg", "SVG image")
        } else if lower.starts_with("<!doctype html") || lower.starts_with("<html") {
            sniffed("text/html", "html", "HTML document")
        } else if lower.starts_with("mime-version:") || lower.contains("\nmime-version:") {
            sniffed("message/rfc822", "mht", "MIME message")
        } else if lower.starts_with("<?xml") {
            sniffed("application/xml", "xml", "XML document")
        } else {
            sniffed("text/plain", "txt", "plain text")
        },
    )
}

/// Media types and their usual extensions. The first entry for a media
/// type gives its extension; the first entry for an extension gives its
/// media type.
const MIME_EXTENSIONS: &[(&str, &str)] = &[
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/jpeg", "jpeg"),
    ("image/jpg", "jpg"),
    ("image/pjpeg", "jpg"),
    ("image/gif", "gif"),
    ("image/bmp", "bmp"),
    ("image/x-bmp", "bmp"),
    ("image/x-ms-bmp", "bmp"),
    ("image/tiff", "tif"),
    ("image/tiff", "tiff"),
    ("image/webp", "webp"),
    ("image/x-icon", "ico"),
    ("image/x-emf", "emf"),
    ("image/emf", "emf"),
    ("image/x-wmf", "wmf"),
    ("image/wmf", "wmf"),
    ("image/x-pict", "pict"),
    ("image/svg+xml", "svg"),
    ("application/pdf", "pdf"),
    ("application/rtf", "rtf"),
    ("text/rtf", "rtf"),
    ("application/zip", "zip"),
    ("application/x-ole-storage", "ole"),
    ("text/html", "html"),
    ("text/html", "htm"),
    ("text/plain", "txt"),
    ("text/csv", "csv"),
    ("application/xml", "xml"),
    ("text/xml", "xml"),
    ("message/rfc822", "eml"),
    ("audio/mpeg", "mp3"),
    ("audio/wav", "wav"),
    ("audio/x-wav", "wav"),
    ("audio/mp4", "m4a"),
    ("audio/x-ms-wma", "wma"),
    ("audio/midi", "mid"),
    ("video/mp4", "mp4"),
    ("video/quicktime", "mov"),
    ("video/x-msvideo", "avi"),
    ("video/x-ms-wmv", "wmv"),
    ("video/mpeg", "mpg"),
    ("application/msword", "doc"),
    ("application/vnd.ms-excel", "xls"),
    ("application/vnd.ms-powerpoint", "ppt"),
    ("application/vnd.visio", "vsd"),
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "docx",
    ),
    (
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsx",
    ),
    (
        "application/vnd.ms-excel.sheet.binary.macroenabled.12",
        "xlsb",
    ),
    (
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "pptx",
    ),
    ("application/vnd.ms-visio.drawing", "vsdx"),
];

/// The usual extension for a media type, if known.
pub fn extension_for_mime(mime: &str) -> Option<&'static str> {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or(mime)
        .trim()
        .to_ascii_lowercase();
    MIME_EXTENSIONS
        .iter()
        .find(|(m, _)| *m == mime)
        .map(|(_, ext)| *ext)
}

/// The media type usually meant by a file extension (without the dot), if
/// known.
pub fn mime_for_extension(extension: &str) -> Option<&'static str> {
    let extension = extension.trim_start_matches('.').to_ascii_lowercase();
    MIME_EXTENSIONS
        .iter()
        .find(|(_, e)| *e == extension)
        .map(|(mime, _)| *mime)
}

/// Pixel dimensions of an image, if its header can be read.
pub fn image_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let size = imagesize::blob_size(data).ok()?;
    let w = u32::try_from(size.width).ok()?;
    let h = u32::try_from(size.height).ok()?;
    (w > 0 && h > 0).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_common_signatures() {
        assert_eq!(sniff(b"%PDF-1.7\n...").unwrap().extension, "pdf");
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n\0\0").unwrap().mime, "image/png");
        assert_eq!(sniff(b"PK\x03\x04rest").unwrap().extension, "zip");
        assert_eq!(sniff(b"<!DOCTYPE html><p>").unwrap().extension, "html");
        assert_eq!(
            sniff("plain words, ünïcode".as_bytes()).unwrap().mime,
            "text/plain"
        );
        assert_eq!(sniff(&[0, 1, 2, 3, 0xFF]), None);
    }

    #[test]
    fn maps_mime_to_extension() {
        assert_eq!(extension_for_mime("image/jpeg"), Some("jpg"));
        assert_eq!(extension_for_mime("TEXT/HTML; charset=utf-8"), Some("html"));
        assert_eq!(extension_for_mime("application/x-unknown"), None);
        assert_eq!(mime_for_extension("MP3"), Some("audio/mpeg"));
        assert_eq!(mime_for_extension("jpeg"), Some("image/jpeg"));
    }
}
