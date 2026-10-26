//! PowerPoint presentations (PresentationML).
//!
//! Slides are read in presentation order. The title placeholder names the
//! slide; other placeholders, text boxes, tables, pictures, charts, SmartArt
//! and embedded objects follow in z-order. Speaker notes and review
//! comments are attached to their slide.

use std::collections::HashMap;

use doctopus_core::{
    Block, Document, Group, GroupKind, Input, Note, NoteKind, Paragraph, Result, Section,
    SectionKind, Style, blocks_plain_text, push_text,
};

use super::context::Context;
use super::drawing::{self, Bullets};
use super::package::Relationships;
use super::short_datetime;
use super::xml::Element;
use crate::blocks::push_text_box;

/// Read a presentation.
pub(crate) fn read(input: &Input<'_>) -> Result<Document> {
    let mut cx = Context::open(input)?;
    let (main, pres) = cx.main_xml()?;
    let rels = cx.rels(&main);
    let mut reader = SlideReader {
        sections: section_names(&pres),
        legacy_authors: authors(&mut cx, &rels, "commentAuthors", "p:cmAuthor"),
        modern_authors: authors(&mut cx, &rels, "authors", "p188:author"),
        comments: 0,
        layouts: HashMap::new(),
    };
    let slide_ids: Vec<(Option<String>, String)> = pres
        .path("p:sldIdLst")
        .map(|list| {
            list.children_named("p:sldId")
                .filter_map(|s| {
                    let target = rels.target(s.attr("r:id")?)?.to_string();
                    Some((s.attr("id").map(str::to_string), target))
                })
                .collect()
        })
        .unwrap_or_default();
    for (i, (id, target)) in slide_ids.iter().enumerate() {
        let number = u32::try_from(i + 1).unwrap_or(u32::MAX);
        let mut section = reader.slide(&mut cx, target, number);
        if let Some(name) = id.as_deref().and_then(|id| reader.sections.get(id)) {
            section.properties.insert("section".into(), name.clone());
        }
        cx.doc.sections.push(section);
    }
    Ok(cx.doc)
}

struct SlideReader {
    /// PowerPoint section name by slide id.
    sections: HashMap<String, String>,
    legacy_authors: HashMap<String, String>,
    modern_authors: HashMap<String, String>,
    /// Comments numbered so far, for unique labels.
    comments: u32,
    /// Bullet defaults of layout placeholders, by layout part.
    layouts: HashMap<String, LayoutBullets>,
}

impl SlideReader {
    fn slide(&mut self, cx: &mut Context<'_>, part: &str, number: u32) -> Section {
        let mut section = Section::new(SectionKind::Slide).with_number(number);
        let Some(root) = cx.xml(part) else {
            cx.doc.warn(format!("slide {number} is missing (`{part}`)"));
            return section;
        };
        section.hidden = root.attr_bool("show") == Some(false);
        let rels = cx.rels(part);
        let location = format!("slide {number}");
        let layout = self.layout_bullets(cx, &rels);
        let mut title: Option<String> = None;
        let mut blocks = Vec::new();
        if let Some(tree) = root.path("p:cSld/p:spTree") {
            for shape in tree.children() {
                if !shape.is("p:sp") {
                    drawing::shape(cx, &rels, shape, &location, &mut blocks);
                    continue;
                }
                let Some(body) = shape.child("p:txBody") else {
                    continue;
                };
                let placeholder = shape.path("p:nvSpPr/p:nvPr/p:ph");
                let Some(ph) = placeholder else {
                    let text = drawing::text_body(
                        &rels,
                        body,
                        own_bullets(body).unwrap_or(Bullets::Explicit),
                    );
                    push_text_box(&mut blocks, text);
                    continue;
                };
                // A placeholder without a type is a content placeholder.
                let kind = ph.attr("type").unwrap_or("obj");
                match kind {
                    "title" | "ctrTitle" if title.is_none() => {
                        let text =
                            blocks_plain_text(&drawing::text_body(&rels, body, Bullets::Explicit));
                        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                        if !text.is_empty() {
                            title = Some(text);
                        }
                    }
                    // Repeated on every slide and rarely informative.
                    "dt" | "ftr" | "sldNum" | "hdr" => {}
                    _ => {
                        let bullets = own_bullets(body).unwrap_or_else(|| {
                            if matches!(kind, "body" | "obj") {
                                layout.for_placeholder(ph)
                            } else {
                                Bullets::Explicit
                            }
                        });
                        blocks.extend(drawing::text_body(&rels, body, bullets));
                    }
                }
            }
        }
        if let Some(title) = title {
            section = section.with_name(title);
        }
        section.blocks = blocks;
        if let Some(notes) = rels.first_target("notesSlide").map(str::to_string)
            && let Some(group) = notes_group(cx, &notes)
        {
            section.blocks.push(group);
        }
        for target in rels
            .of_kind("comments")
            .filter(|r| !r.external)
            .map(|r| r.target.clone())
            .collect::<Vec<_>>()
        {
            let notes = self.comments(cx, &target);
            section.blocks.extend(notes);
        }
        section
    }

    /// Whether body placeholders of the slide's layout are bulleted.
    fn layout_bullets(&mut self, cx: &mut Context<'_>, rels: &Relationships) -> LayoutBullets {
        let Some(layout) = rels.first_target("slideLayout").map(str::to_string) else {
            return LayoutBullets::default();
        };
        if let Some(cached) = self.layouts.get(&layout) {
            return cached.clone();
        }
        let mut info = LayoutBullets::default();
        let layout_rels = cx.rels(&layout);
        if let Some(master) = layout_rels.first_target("slideMaster").map(str::to_string)
            && let Some(root) = cx.xml(&master)
        {
            let level1 = root.path("p:txStyles/p:bodyStyle/a:lvl1pPr");
            info.master = level1.and_then(level_bullets).unwrap_or(Bullets::Inherited);
        }
        if let Some(root) = cx.xml(&layout) {
            for sp in root.find_all("p:sp") {
                let Some(ph) = sp.path("p:nvSpPr/p:nvPr/p:ph") else {
                    continue;
                };
                let Some(bullets) = sp.child("p:txBody").and_then(own_bullets) else {
                    continue;
                };
                if let Some(idx) = ph.attr("idx") {
                    info.by_idx.insert(idx.to_string(), bullets);
                }
            }
        }
        self.layouts.insert(layout, info.clone());
        info
    }

    fn comments(&mut self, cx: &mut Context<'_>, part: &str) -> Vec<Block> {
        let Some(root) = cx.xml(part) else {
            return Vec::new();
        };
        let rels = cx.rels(part);
        let mut out = Vec::new();
        if root.is("p:cmLst") {
            for cm in root.children_named("p:cm") {
                let text = cm.child("p:text").map(|t| t.text()).unwrap_or_default();
                let author = cm
                    .attr("authorId")
                    .and_then(|id| self.legacy_authors.get(id))
                    .cloned();
                out.push(self.note(author, cm.attr("dt"), vec![Block::text(text.trim())]));
            }
        } else if root.is("p188:cmLst") {
            for cm in root.children_named("p188:cm") {
                let mut blocks = cm
                    .child("p188:txBody")
                    .map(|b| drawing::text_body(&rels, b, Bullets::Explicit))
                    .unwrap_or_default();
                for reply in cm
                    .path("p188:replyLst")
                    .into_iter()
                    .flat_map(|l| l.children_named("p188:reply"))
                {
                    let who = reply
                        .attr("authorId")
                        .and_then(|id| self.modern_authors.get(id))
                        .map_or("reply", String::as_str);
                    let text = reply
                        .child("p188:txBody")
                        .map(|b| {
                            blocks_plain_text(&drawing::text_body(&rels, b, Bullets::Explicit))
                        })
                        .unwrap_or_default();
                    let mut content = Vec::new();
                    push_text(&mut content, "Reply from ", Style::default());
                    push_text(
                        &mut content,
                        who,
                        Style {
                            bold: true,
                            ..Style::default()
                        },
                    );
                    if let Some(date) = reply.attr("created") {
                        push_text(
                            &mut content,
                            &format!(" ({})", short_datetime(date)),
                            Style::default(),
                        );
                    }
                    push_text(
                        &mut content,
                        &format!(": {}", text.trim()),
                        Style::default(),
                    );
                    blocks.push(Block::Paragraph(Paragraph { content }));
                }
                if cm.attr("status") == Some("resolved") {
                    blocks.push(Block::text("(resolved)"));
                }
                let author = cm
                    .attr("authorId")
                    .and_then(|id| self.modern_authors.get(id))
                    .cloned();
                out.push(self.note(author, cm.attr("created"), blocks));
            }
        }
        out
    }

    fn note(&mut self, author: Option<String>, date: Option<&str>, blocks: Vec<Block>) -> Block {
        self.comments += 1;
        Block::Note(Note {
            kind: NoteKind::Comment,
            label: format!("c{}", self.comments),
            author,
            date: date.map(short_datetime),
            anchor: None,
            blocks,
        })
    }
}

/// Bullet defaults of one slide layout.
#[derive(Debug, Clone)]
struct LayoutBullets {
    /// From the master's body text style.
    master: Bullets,
    /// Overrides of individual layout placeholders, by `idx`.
    by_idx: HashMap<String, Bullets>,
}

impl Default for LayoutBullets {
    fn default() -> Self {
        Self {
            master: Bullets::Inherited,
            by_idx: HashMap::new(),
        }
    }
}

impl LayoutBullets {
    fn for_placeholder(&self, ph: &Element) -> Bullets {
        ph.attr("idx")
            .and_then(|idx| self.by_idx.get(idx))
            .copied()
            .unwrap_or(self.master)
    }
}

/// Bullet formatting set by a text body's own list style, if any.
fn own_bullets(body: &Element) -> Option<Bullets> {
    body.path("a:lstStyle/a:lvl1pPr").and_then(level_bullets)
}

fn level_bullets(level: &Element) -> Option<Bullets> {
    if level.child("a:buNone").is_some() {
        Some(Bullets::Explicit)
    } else if level.child("a:buChar").is_some() || level.child("a:buAutoNum").is_some() {
        Some(Bullets::Inherited)
    } else {
        None
    }
}

fn notes_group(cx: &mut Context<'_>, part: &str) -> Option<Block> {
    let root = cx.xml(part)?;
    let rels = cx.rels(part);
    let tree = root.path("p:cSld/p:spTree")?;
    let mut blocks = Vec::new();
    for sp in tree.children_named("p:sp") {
        let is_body = sp
            .path("p:nvSpPr/p:nvPr/p:ph")
            .is_some_and(|ph| ph.attr("type") == Some("body"));
        if !is_body {
            continue;
        }
        if let Some(body) = sp.child("p:txBody") {
            blocks.extend(drawing::text_body(&rels, body, Bullets::Explicit));
        }
    }
    let has_text = blocks.iter().any(|b| !b.is_blank());
    has_text.then(|| {
        Block::Group(Group {
            kind: GroupKind::Notes,
            title: Some("Notes".into()),
            blocks,
        })
    })
}

/// PowerPoint sections (`p14:sectionLst`): section name by slide id.
fn section_names(pres: &Element) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for section in pres.find_all("p14:section") {
        let Some(name) = section.attr("name") else {
            continue;
        };
        for id in section.find_all("p14:sldId") {
            if let Some(id) = id.attr("id") {
                out.insert(id.to_string(), name.to_string());
            }
        }
    }
    out
}

/// Comment authors: id → name.
fn authors(
    cx: &mut Context<'_>,
    rels: &Relationships,
    kind: &str,
    element: &str,
) -> HashMap<String, String> {
    let Some(target) = rels.first_target(kind).map(str::to_string) else {
        return HashMap::new();
    };
    let Some(root) = cx.xml(&target) else {
        return HashMap::new();
    };
    root.children_named(element)
        .filter_map(|a| Some((a.attr("id")?.to_string(), a.attr("name")?.to_string())))
        .collect()
}
