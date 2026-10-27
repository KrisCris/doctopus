//! Word documents (WordprocessingML).
//!
//! The body is read in order, with headings and list labels resolved from
//! styles and numbering definitions. Headers and footers, footnotes,
//! endnotes and comments follow as sections of their own; references to
//! notes and comments stay in the text.

mod body;
mod styles;

use doctopus_core::{
    Block, Document, Group, GroupKind, Input, Note, NoteKind, Result, Section, SectionKind,
    blocks_plain_text,
};

use self::body::Walker;
use self::styles::Styles;
use super::context::Context;
use super::short_datetime;
use super::xml::Element;
use crate::numbering::Numbering;
use crate::word::{anchor_text, keep_heading_links, prepend, record_tracked_changes};

/// Read a Word document.
pub(crate) fn read(input: &Input<'_>) -> Result<Document> {
    let mut cx = Context::open(input)?;
    let (main, root) = cx.main_xml()?;
    let rels = cx.rels(&main);
    let styles = match rels.first_target("styles").map(str::to_string) {
        Some(part) => cx.xml(&part).map(|r| Styles::parse(&r)).unwrap_or_default(),
        None => Styles::default(),
    };
    let numbering = match rels.first_target("numbering").map(str::to_string) {
        Some(part) => cx
            .xml(&part)
            .map(|r| Numbering::parse(&r, &styles))
            .unwrap_or_default(),
        None => Numbering::default(),
    };

    let mut walker = Walker::new(&mut cx, &styles, &numbering, rels.clone());
    let mut body = Section::new(SectionKind::Body);
    if let Some(content) = root.child("w:body") {
        walker.blocks(content, &mut body.blocks);
    }
    let mut sections = vec![body];
    if let Some(section) = headers_footers(&mut walker, &root) {
        sections.push(section);
    }
    let footnotes = rels.first_target("footnotes").map(str::to_string);
    if let Some(section) = notes(&mut walker, footnotes, NoteKind::Footnote) {
        sections.push(section);
    }
    let endnotes = rels.first_target("endnotes").map(str::to_string);
    if let Some(section) = notes(&mut walker, endnotes, NoteKind::Endnote) {
        sections.push(section);
    }
    let comments = rels.first_target("comments").map(str::to_string);
    let extended = rels.first_target("commentsExtended").map(str::to_string);
    if let Some(section) = comments_section(&mut walker, comments, extended) {
        sections.push(section);
    }
    let (insertions, deletions) = (walker.insertions, walker.deletions);
    drop(walker);
    record_tracked_changes(&mut cx.doc, insertions, deletions);
    keep_heading_links(&mut sections);
    cx.doc.sections = sections;
    Ok(cx.doc)
}

/// Headers and footers referenced by the document's section properties,
/// each part once.
fn headers_footers(walker: &mut Walker<'_, '_>, root: &Element) -> Option<Section> {
    let body_rels = walker.rels.clone();
    let mut seen: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut section = Section::new(SectionKind::HeadersFooters);
    for sect in root.find_all("w:sectPr") {
        for reference in sect.children() {
            let kind = match reference.name() {
                "w:headerReference" => GroupKind::Header,
                "w:footerReference" => GroupKind::Footer,
                _ => continue,
            };
            let Some(target) = reference
                .attr("r:id")
                .and_then(|id| body_rels.target(id))
                .map(str::to_string)
            else {
                continue;
            };
            if seen.contains(&target) {
                continue;
            }
            seen.push(target.clone());
            let Some(part) = walker.cx.xml(&target) else {
                continue;
            };
            walker.rels = walker.cx.rels(&target);
            walker.area = if kind == GroupKind::Header {
                "header"
            } else {
                "footer"
            }
            .into();
            let mut blocks = Vec::new();
            walker.blocks(&part, &mut blocks);
            if blocks.iter().all(Block::is_blank) {
                continue;
            }
            // The same header text is often stored once per section.
            let text = blocks_plain_text(&blocks).trim().to_string();
            if !text.is_empty() {
                if texts.contains(&text) {
                    continue;
                }
                texts.push(text);
            }
            let title = match reference.attr("w:type") {
                Some("first") => Some("first page".to_string()),
                Some("even") => Some("even pages".to_string()),
                _ => None,
            };
            section.blocks.push(Block::Group(Group {
                kind,
                title,
                blocks,
            }));
        }
    }
    walker.rels = body_rels;
    walker.area = "body".into();
    (!section.blocks.is_empty()).then_some(section)
}

/// Footnotes or endnotes, in the order they are referenced.
fn notes(walker: &mut Walker<'_, '_>, part: Option<String>, kind: NoteKind) -> Option<Section> {
    let part = part?;
    let root = walker.cx.xml(&part)?;
    let saved = std::mem::replace(&mut walker.rels, walker.cx.rels(&part));
    let (element, area, section_kind) = match kind {
        NoteKind::Endnote => ("w:endnote", "endnotes", SectionKind::Endnotes),
        _ => ("w:footnote", "footnotes", SectionKind::Footnotes),
    };
    walker.area = area.into();
    let notes: Vec<&Element> = root
        .children_named(element)
        .filter(|n| matches!(n.attr("w:type"), None | Some("normal")))
        .collect();
    let referenced: Vec<(String, String)> = match kind {
        NoteKind::Endnote => walker.notes.endnotes.clone(),
        _ => walker.notes.footnotes.clone(),
    };
    let mut ordered: Vec<(&Element, String)> = Vec::new();
    for (id, label) in &referenced {
        if let Some(note) = notes.iter().find(|n| n.attr("w:id") == Some(id.as_str())) {
            ordered.push((note, label.clone()));
        }
    }
    for note in &notes {
        let id = note.attr("w:id").unwrap_or_default();
        if !referenced.iter().any(|(i, _)| i == id) {
            let label = match kind {
                NoteKind::Endnote => walker.notes.endnote(id),
                _ => walker.notes.footnote(id),
            };
            ordered.push((note, label));
        }
    }
    let mut section = Section::new(section_kind);
    for (note, label) in ordered {
        let mut blocks = Vec::new();
        walker.blocks(note, &mut blocks);
        if blocks.iter().all(Block::is_blank) {
            continue;
        }
        section.blocks.push(Block::Note(Note {
            kind,
            label,
            author: None,
            date: None,
            anchor: None,
            blocks,
        }));
    }
    walker.rels = saved;
    walker.area = "body".into();
    (!section.blocks.is_empty()).then_some(section)
}

/// Review comments with the text they are anchored to, replies marked.
fn comments_section(
    walker: &mut Walker<'_, '_>,
    part: Option<String>,
    extended: Option<String>,
) -> Option<Section> {
    let part = part?;
    let root = walker.cx.xml(&part)?;
    // commentsExtended links comments (by the id of their last paragraph)
    // to their parent and records whether they are resolved.
    let mut threads: Vec<(String, Option<String>, bool)> = Vec::new();
    if let Some(ext) = extended.and_then(|p| walker.cx.xml(&p)) {
        for ex in ext.children().filter(|e| e.local() == "commentEx") {
            if let Some(para) = ex.attr("w15:paraId") {
                threads.push((
                    para.to_string(),
                    ex.attr("w15:paraIdParent").map(str::to_string),
                    ex.attr_bool("w15:done").unwrap_or(false),
                ));
            }
        }
    }
    let saved = std::mem::replace(&mut walker.rels, walker.cx.rels(&part));
    walker.area = "comments".into();
    let comments: Vec<&Element> = root.children_named("w:comment").collect();
    let para_of = |c: &Element| {
        c.children_named("w:p")
            .last()
            .and_then(|p| p.attr("w14:paraId"))
            .map(str::to_string)
    };
    let mut entries: Vec<(String, Block)> = Vec::new();
    for comment in &comments {
        let Some(id) = comment.attr("w:id") else {
            continue;
        };
        let label = walker.notes.comment(id);
        let mut blocks = Vec::new();
        walker.blocks(comment, &mut blocks);
        let thread = para_of(comment).and_then(|p| threads.iter().find(|(para, _, _)| *para == p));
        if let Some((_, Some(parent), _)) = thread {
            let parent_label = comments
                .iter()
                .find(|c| para_of(c).as_deref() == Some(parent.as_str()))
                .and_then(|c| c.attr("w:id"))
                .map(|pid| walker.notes.comment(pid));
            if let Some(parent_label) = parent_label {
                prepend(&mut blocks, &format!("(reply to {parent_label}) "));
            }
        }
        if thread.is_some_and(|(_, _, done)| *done) {
            blocks.push(Block::text("(resolved)"));
        }
        let anchor = walker.anchors.get(id).and_then(|a| anchor_text(a));
        entries.push((
            label.clone(),
            Block::Note(Note {
                kind: NoteKind::Comment,
                label,
                author: comment
                    .attr("w:author")
                    .map(str::to_string)
                    .filter(|a| !a.trim().is_empty()),
                date: comment.attr("w:date").map(short_datetime),
                anchor,
                blocks,
            }),
        ));
    }
    walker.rels = saved;
    walker.area = "body".into();
    // Order by label number, i.e. by first reference in the text.
    entries.sort_by_key(|(label, _)| {
        label
            .trim_start_matches('c')
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    let mut section = Section::new(SectionKind::Comments);
    section.blocks = entries.into_iter().map(|(_, b)| b).collect();
    (!section.blocks.is_empty()).then_some(section)
}
