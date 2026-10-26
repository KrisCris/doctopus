//! Conventions of Word shared by the `.docx` and `.doc` readers: field
//! instructions, the characters of symbol fonts, and how paragraphs become
//! headings, list items and paragraphs.

use doctopus_core::{Block, Document, Inline, ListItem, plain_text};

use crate::numbering::ListLabel;

/// Longest comment anchor text kept.
pub(crate) const MAX_ANCHOR: usize = 200;
/// Longest heading text used to say where a part is.
const MAX_HEADING: usize = 80;

/// The block for a paragraph's content, given its heading level and list
/// label, plus the heading text when it is a heading. Numbered headings
/// keep their number.
pub(crate) fn paragraph_block(
    mut content: Vec<Inline>,
    heading: Option<u8>,
    label: ListLabel,
    list_level: u8,
) -> (Block, Option<String>) {
    if let Some(level) = heading {
        let text = plain_text(&content)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if let ListLabel::Number(n) = label {
            content.insert(0, Inline::text(format!("{n} ")));
        }
        let heading = text.chars().take(MAX_HEADING).collect();
        return (Block::heading(level, content), Some(heading));
    }
    let block = match label {
        ListLabel::None => Block::paragraph(content),
        ListLabel::Bullet => list_item(list_level, None, content),
        ListLabel::Number(n) => list_item(list_level, Some(n), content),
    };
    (block, None)
}

fn list_item(level: u8, label: Option<String>, content: Vec<Inline>) -> Block {
    Block::ListItem(ListItem {
        level: level.min(8),
        label,
        content,
    })
}

/// Whether inline content shows nothing.
pub(crate) fn is_blank(content: &[Inline]) -> bool {
    content.iter().all(|i| match i {
        Inline::Text(t) => t.text.trim().is_empty(),
        Inline::LineBreak => true,
        Inline::Link(l) => is_blank(&l.content),
        Inline::NoteRef(_) | Inline::Image(_) | Inline::Math(_) => false,
    })
}

/// Drop leading and trailing line breaks (Word users end paragraphs with
/// them surprisingly often).
pub(crate) fn trim_inlines(content: &mut Vec<Inline>) {
    while matches!(content.last(), Some(Inline::LineBreak)) {
        content.pop();
    }
    while matches!(content.first(), Some(Inline::LineBreak)) {
        content.remove(0);
    }
}

/// Put text in front of a note's content (`(reply to c1) `).
pub(crate) fn prepend(blocks: &mut Vec<Block>, text: &str) {
    match blocks.first_mut() {
        Some(Block::Paragraph(p)) => p.content.insert(0, Inline::text(text)),
        _ => blocks.insert(0, Block::text(text.trim())),
    }
}

/// A comment anchor as one line of limited length.
pub(crate) fn anchor_text(text: &str) -> Option<String> {
    let text = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_ANCHOR)
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}

/// Say in the metadata that the text has tracked changes, and how they
/// were applied.
pub(crate) fn record_tracked_changes(doc: &mut Document, insertions: usize, deletions: usize) {
    if insertions + deletions > 0 {
        doc.metadata.set(
            "tracked_changes",
            format!("{insertions} insertions, {deletions} deletions (text is shown with insertions applied and deletions removed)"),
        );
    }
}

/// The target of a `HYPERLINK "url"` field instruction. Links to
/// bookmarks (`\l`) are not kept.
pub(crate) fn hyperlink_target(instruction: &str) -> Option<String> {
    let rest = instruction.trim().strip_prefix("HYPERLINK")?.trim();
    if rest.starts_with("\\l") {
        return None;
    }
    let target = if let Some(quoted) = rest.strip_prefix('"') {
        quoted.split('"').next().unwrap_or_default()
    } else {
        rest.split_whitespace().next().unwrap_or_default()
    };
    let target = target.trim();
    (!target.is_empty()).then(|| target.to_string())
}

/// A character in a symbol font. Symbol-font code points (`F0xx`) are
/// mapped to Unicode where the meaning is clear and dropped otherwise.
pub(crate) fn symbol_char(font: &str, code: u32) -> Option<char> {
    let font = font.to_ascii_lowercase();
    let code = if (0xF000..=0xF0FF).contains(&code) {
        code - 0xF000
    } else {
        code
    };
    if font == "symbol" {
        return symbol_font(code);
    }
    if font.starts_with("wingdings") {
        return match code {
            0xFC | 0xFE => Some('\u{2713}'),
            0xFB | 0xFD => Some('\u{2717}'),
            0xA7 | 0x6C | 0x6E | 0x9F => Some('\u{2022}'),
            0xE0 | 0xE8 => Some('\u{2192}'),
            _ => None,
        };
    }
    char::from_u32(code).filter(|c| !c.is_control())
}

/// The Symbol font (Adobe Symbol encoding): Greek letters, math signs,
/// arrows and the pieces of large brackets.
fn symbol_font(code: u32) -> Option<char> {
    /// 0x20 to 0x7E.
    const LOW: &str = " !\u{2200}#\u{2203}%&\u{220B}()\u{2217}+,\u{2212}./0123456789:;<=>?\
        \u{2245}\u{391}\u{392}\u{3A7}\u{394}\u{395}\u{3A6}\u{393}\u{397}\u{399}\u{3D1}\u{39A}\
        \u{39B}\u{39C}\u{39D}\u{39F}\u{3A0}\u{398}\u{3A1}\u{3A3}\u{3A4}\u{3A5}\u{3C2}\u{3A9}\
        \u{39E}\u{3A8}\u{396}[\u{2234}]\u{22A5}_\u{203E}\u{3B1}\u{3B2}\u{3C7}\u{3B4}\u{3B5}\
        \u{3C6}\u{3B3}\u{3B7}\u{3B9}\u{3D5}\u{3BA}\u{3BB}\u{3BC}\u{3BD}\u{3BF}\u{3C0}\u{3B8}\
        \u{3C1}\u{3C3}\u{3C4}\u{3C5}\u{3D6}\u{3C9}\u{3BE}\u{3C8}\u{3B6}{|}\u{223C}";
    /// 0xA0 to 0xFE; NUL marks the one unassigned code.
    const HIGH: &str = "\u{20AC}\u{3D2}\u{2032}\u{2264}\u{2044}\u{221E}\u{192}\u{2663}\u{2666}\
        \u{2665}\u{2660}\u{2194}\u{2190}\u{2191}\u{2192}\u{2193}\u{B0}\u{B1}\u{2033}\u{2265}\
        \u{D7}\u{221D}\u{2202}\u{2022}\u{F7}\u{2260}\u{2261}\u{2248}\u{2026}\u{23D0}\u{23AF}\
        \u{21B5}\u{2135}\u{2111}\u{211C}\u{2118}\u{2297}\u{2295}\u{2205}\u{2229}\u{222A}\
        \u{2283}\u{2287}\u{2284}\u{2282}\u{2286}\u{2208}\u{2209}\u{2220}\u{2207}\u{AE}\u{A9}\
        \u{2122}\u{220F}\u{221A}\u{22C5}\u{AC}\u{2227}\u{2228}\u{21D4}\u{21D0}\u{21D1}\u{21D2}\
        \u{21D3}\u{25CA}\u{2329}\u{AE}\u{A9}\u{2122}\u{2211}\u{239B}\u{239C}\u{239D}\u{23A1}\
        \u{23A2}\u{23A3}\u{23A7}\u{23A8}\u{23A9}\u{23AA}\0\u{232A}\u{222B}\u{2320}\u{23AE}\
        \u{2321}\u{239E}\u{239F}\u{23A0}\u{23A4}\u{23A5}\u{23A6}\u{23AB}\u{23AC}\u{23AD}";
    let c = match code {
        0x20..=0x7E => LOW.chars().nth((code - 0x20) as usize),
        0xA0..=0xFE => HIGH.chars().nth((code - 0xA0) as usize),
        _ => None,
    };
    c.filter(|&c| c != '\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyperlink_instructions() {
        assert_eq!(
            hyperlink_target(r#" HYPERLINK "https://example.com/a b" \o "tip" "#).as_deref(),
            Some("https://example.com/a b")
        );
        assert_eq!(
            hyperlink_target("HYPERLINK http://x.y").as_deref(),
            Some("http://x.y")
        );
        assert_eq!(hyperlink_target(r#"HYPERLINK \l "_Toc1""#), None);
        assert_eq!(hyperlink_target("PAGE"), None);
    }

    #[test]
    fn symbol_fonts_map_to_unicode() {
        assert_eq!(symbol_char("Symbol", 0xF061), Some('α'));
        assert_eq!(symbol_char("Wingdings", 0xF0FC), Some('✓'));
        assert_eq!(symbol_char("Arial", 0x2014), Some('—'));
        assert_eq!(symbol_char("Symbol", 0xF0B0), Some('°'));
        assert_eq!(symbol_char("Symbol", 0x22), Some('∀'));
        assert_eq!(symbol_char("Symbol", 0x7E), Some('∼'));
        assert_eq!(symbol_char("Symbol", 0xFE), Some('⎭'));
        assert_eq!(symbol_char("Symbol", 0xF0), None);
    }
}
