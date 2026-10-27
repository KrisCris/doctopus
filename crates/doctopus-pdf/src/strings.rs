//! PDF text strings and dates.

use hayro_syntax::object::DateTime;

/// Decode a PDF text string: UTF-16BE or UTF-8 when it starts with a byte
/// order mark, PDFDocEncoding otherwise. Some writers leave the mark out
/// of UTF-8, so a string that is valid UTF-8 and not ASCII is read as
/// UTF-8: PDFDocEncoding text is rarely valid UTF-8.
pub(crate) fn text(bytes: &[u8]) -> String {
    let text = if let Some(utf16) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let (pairs, _) = utf16.as_chunks::<2>();
        let units: Vec<u16> = pairs.iter().map(|pair| u16::from_be_bytes(*pair)).collect();
        char::decode_utf16(units)
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    } else if let Some(utf8) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(utf8).into_owned()
    } else if let Ok(utf8) = std::str::from_utf8(bytes)
        && !utf8.is_ascii()
    {
        utf8.to_string()
    } else {
        bytes.iter().map(|&b| pdf_doc_char(b)).collect()
    };
    // Language tags (ESC ... ESC) and NUL padding are not text.
    strip_escapes(&text)
        .chars()
        .filter(|&c| c != '\0')
        .collect()
}

/// Drop the language-tag escapes of a text string (PDF 32000-1, 7.9.2.2).
fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_escape = false;
    for c in text.chars() {
        if c == '\u{1B}' {
            in_escape = !in_escape;
        } else if !in_escape {
            out.push(c);
        }
    }
    out
}

/// One byte of PDFDocEncoding (PDF 32000-1, Annex D). It is Latin-1
/// except for 0x18-0x1F and 0x80-0xA0.
fn pdf_doc_char(byte: u8) -> char {
    const LOW: [char; 8] = ['˘', 'ˇ', 'ˆ', '˙', '˝', '˛', '˚', '˜'];
    const HIGH: [char; 33] = [
        '•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘', '’', '‚',
        '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ', 'Š', 'Ÿ', 'Ž', 'ı', 'ł', 'œ', 'š', 'ž', '\u{FFFD}', '€',
    ];
    match byte {
        0x18..=0x1F => LOW[usize::from(byte - 0x18)],
        0x80..=0xA0 => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

/// A PDF date as ISO 8601. A zero offset is left out: the parser reports
/// it both for UTC and for dates that give no time zone.
pub(crate) fn date(date: &DateTime) -> String {
    let mut out = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        date.year, date.month, date.day, date.hour, date.minute, date.second
    );
    let (hours, minutes) = (date.utc_offset_hour, date.utc_offset_minute);
    if hours != 0 || minutes != 0 {
        let sign = if hours < 0 { '-' } else { '+' };
        out.push_str(&format!("{sign}{:02}:{minutes:02}", hours.unsigned_abs()));
    }
    out
}

/// The last component of a file name that may carry a path.
pub(crate) fn file_name(name: &str) -> &str {
    name.rsplit(['/', '\\', ':'])
        .find(|s| !s.trim().is_empty())
        .unwrap_or(name)
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_strings_are_decoded() {
        assert_eq!(text(b"Caf\xe9 \x84 \x93"), "Café — ﬁ");
        assert_eq!(
            text(&[0xFE, 0xFF, 0x00, 0x48, 0x00, 0xE9, 0x4E, 0x2D]),
            "Hé中"
        );
        assert_eq!(text(&[0xEF, 0xBB, 0xBF, b'o', b'k']), "ok");
        assert_eq!(text(b"Delacr\xc3\xa9taz"), "Delacrétaz");
        // A language tag and a lone surrogate.
        assert_eq!(
            text(&[
                0xFE, 0xFF, 0x00, 0x1B, 0x00, b'e', 0x00, 0x1B, 0x00, b'x', 0xD8, 0x00
            ]),
            "x\u{FFFD}"
        );
        // An odd trailing byte is dropped.
        assert_eq!(text(&[0xFE, 0xFF, 0x00, b'a', 0x00]), "a");
    }

    #[test]
    fn file_names_lose_their_paths() {
        assert_eq!(file_name("C:\\Users\\me\\report.docx"), "report.docx");
        assert_eq!(file_name("/tmp/a.pdf"), "a.pdf");
        assert_eq!(file_name("plain.txt"), "plain.txt");
        assert_eq!(file_name("dir/"), "dir");
    }
}
