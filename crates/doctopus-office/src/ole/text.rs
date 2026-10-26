//! Decoding the 8-bit and UTF-16 strings of binary Office formats.

use encoding_rs::Encoding;

/// Decode UTF-16LE, stopping at the first NUL.
pub(crate) fn utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Decode text in the Windows "ANSI" code page. Without better information
/// Windows-1252 is assumed, which is right for most Western documents.
pub(crate) fn ansi(bytes: &[u8]) -> String {
    encoding_rs::WINDOWS_1252
        .decode_without_bom_handling(bytes)
        .0
        .into_owned()
}

/// The encoding for a Windows code page number, if known.
pub(crate) fn encoding_for_codepage(codepage: u16) -> Option<&'static Encoding> {
    codepage::to_encoding(codepage)
}

/// Decode 8-bit text in the given Windows code page (falls back to
/// Windows-1252 for unknown code pages).
pub(crate) fn decode_codepage(bytes: &[u8], codepage: u16) -> String {
    encoding_for_codepage(codepage)
        .unwrap_or(encoding_rs::WINDOWS_1252)
        .decode_without_bom_handling(bytes)
        .0
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_strings() {
        assert_eq!(utf16le(&[0x68, 0, 0xE9, 0, 0, 0, 0x78, 0]), "hé");
        assert_eq!(ansi(&[0x93, 0x51, 0x94]), "\u{201c}Q\u{201d}");
        assert_eq!(decode_codepage(&[0xC4, 0xE3], 936), "你");
    }
}
