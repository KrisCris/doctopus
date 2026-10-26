//! Block-building conventions shared by the readers, so that the same kind
//! of content looks the same whichever format it came from.

use doctopus_core::{Block, Group, GroupKind, Inline, Link, Paragraph};

/// Add the text of a shape or text box: a single block inline, more as a
/// group.
pub(crate) fn push_text_box(out: &mut Vec<Block>, blocks: Vec<Block>) {
    match blocks.len() {
        0 => {}
        1 => out.extend(blocks),
        _ => out.push(Block::Group(Group {
            kind: GroupKind::TextBox,
            title: None,
            blocks,
        })),
    }
}

/// Make a shape's text link to `url`: a single paragraph becomes the link
/// text, otherwise the target is added after the text.
pub(crate) fn link_shape(blocks: &mut Vec<Block>, url: String) {
    if let [Block::Paragraph(p)] = blocks.as_mut_slice()
        && !p.content.iter().any(|i| matches!(i, Inline::Link(_)))
    {
        let content = std::mem::take(&mut p.content);
        p.content.push(Inline::Link(Link { url, content }));
        return;
    }
    let text = url.clone();
    blocks.push(Block::Paragraph(Paragraph {
        content: vec![
            Inline::text("Link: "),
            Inline::Link(Link {
                url,
                content: vec![Inline::text(text)],
            }),
        ],
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_paragraphs_become_the_link_text() {
        let mut blocks = vec![Block::text("Docs")];
        link_shape(&mut blocks, "https://example.com/".into());
        assert_eq!(blocks.len(), 1);
        assert!(
            matches!(&blocks[0], Block::Paragraph(p) if matches!(&p.content[0], Inline::Link(l) if l.url == "https://example.com/"))
        );

        let mut blocks = vec![Block::text("a"), Block::text("b")];
        link_shape(&mut blocks, "https://example.com/".into());
        assert_eq!(blocks.len(), 3);
    }

    #[test]
    fn text_boxes_group_several_blocks() {
        let mut out = Vec::new();
        push_text_box(&mut out, vec![Block::text("one")]);
        push_text_box(&mut out, vec![Block::text("a"), Block::text("b")]);
        push_text_box(&mut out, Vec::new());
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[1], Block::Group(g) if g.kind == GroupKind::TextBox));
    }
}
