//! Word 97-2003 documents (`.doc`, `.dot`).
//!
//! The `WordDocument` stream starts with the File Information Block (FIB),
//! which locates everything else in the table stream (`0Table` or
//! `1Table`). The text of all stories (main text, footnotes, headers and
//! footers, comments, endnotes and text boxes) is one sequence of
//! characters, assembled by the piece table. Formatting comes from
//! formatted disk pages in the main stream and from the style sheet.
//! Special characters in the text mark fields, pictures (kept in the
//! `Data` stream), embedded objects (in `ObjectPool`), floating drawings
//! (OfficeArt, in the table stream) and note references.
//!
//! The output follows the `.docx` reader: the body first, then headers and
//! footers, footnotes, endnotes and comments as sections of their own.

mod fib;
mod props;
mod story;
mod styles;
mod text;

use std::ops::Range;

use doctopus_core::{
    Block, Document, EmbedRef, Error, Group, GroupKind, Input, Note, NoteKind, Result, Section,
    SectionKind, blocks_plain_text,
};

use self::fib::{Fib, Kind, StoryLengths, lcb};
use self::props::ParaProps;
use self::story::Walker;
use self::styles::Styles;
use self::text::{Bins, Plc, Text};
use super::context::Context;
use super::officeart::{self, BlipStore, Records, Shape, rt};
use crate::binary::{Cursor, i32_at, u16_at, u32_at};
use crate::ole::text::utf16le;
use crate::word::{anchor_text, keep_heading_links, prepend, record_tracked_changes};

/// Read a `.doc` document.
pub(crate) fn read(input: &Input<'_>) -> Result<Document> {
    let mut cx = Context::open(input)?;
    let format = input.format.name;
    let word = cx.required_stream("/WordDocument")?;
    match fib::kind(&word) {
        Kind::Word97 => {}
        Kind::Word6 => {
            return Err(Error::Unsupported {
                detected: Some("Word 6.0/95 document; save it in a newer format to read it".into()),
            });
        }
        Kind::NotWord => {
            return Err(Error::malformed(
                format,
                "the WordDocument stream does not start with a file information block",
            ));
        }
    }
    let fib = Fib::read(&word)
        .ok_or_else(|| Error::malformed(format, "the file information block is damaged"))?;
    if fib.encrypted {
        return Err(Error::encrypted(format));
    }
    let table = cx.required_stream(fib.table_stream)?;
    let data = cx.stream("/Data").unwrap_or_default();
    cx.detect_macros(&["/Macros"]);
    let max = cx.options.max_part_size;
    let doc = Doc::read(&fib, &word, &table, &data, max)
        .ok_or_else(|| Error::malformed(format, "the piece table of the text is damaged"))?;
    let numbering = styles::numbering(
        fib.tail(&table, lcb::PLF_LST),
        fib.tail(&table, lcb::PLF_LFO),
    );

    let mut walker = Walker::new(&mut cx, &doc, &numbering);
    let mut body = Section::new(SectionKind::Body);
    body.blocks = walker.story(Story::Main, doc.stories.range(Story::Main));
    let mut sections = vec![body];
    if let Some(section) = headers_footers(&mut walker, &doc) {
        sections.push(section);
    }
    for kind in [NoteKind::Footnote, NoteKind::Endnote] {
        if let Some(section) = notes(&mut walker, &doc, kind) {
            sections.push(section);
        }
    }
    let comments = Comments::read(&fib, &table);
    if let Some(section) = comments_section(&mut walker, &doc, &comments) {
        sections.push(section);
    }
    let (insertions, deletions) = (walker.insertions, walker.deletions);
    let shown = std::mem::take(&mut walker.objects);
    drop(walker);
    record_tracked_changes(&mut cx.doc, insertions, deletions);
    // Embedded objects the text does not show.
    let orphans: Vec<String> = cx
        .cfb
        .list("/ObjectPool")
        .into_iter()
        .filter(|e| !e.is_stream && !shown.contains(&e.path))
        .map(|e| e.path)
        .collect();
    for path in orphans {
        if let Some(part) = cx.storage_object(&path, None, "document") {
            sections[0].blocks.push(Block::Embed(EmbedRef { part }));
        }
    }
    keep_heading_links(&mut sections);
    cx.doc.sections = sections;
    Ok(cx.doc)
}

/// The stories, in the order their text follows each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Story {
    Main,
    Footnotes,
    Headers,
    Comments,
    Endnotes,
    TextBoxes,
    HeaderTextBoxes,
}

/// Where each story's text is.
#[derive(Debug, Clone, Copy)]
struct Stories {
    /// Start of each story (and of the obsolete macro story), then the end.
    bounds: [u32; 9],
}

impl Stories {
    fn new(ccp: &StoryLengths, len: u32) -> Self {
        let lengths = [
            ccp.text,
            ccp.footnotes,
            ccp.headers,
            ccp.macros,
            ccp.comments,
            ccp.endnotes,
            ccp.text_boxes,
            ccp.header_text_boxes,
        ];
        let mut bounds = [0u32; 9];
        for (i, length) in lengths.iter().enumerate() {
            bounds[i + 1] = bounds[i].saturating_add(*length).min(len);
        }
        Self { bounds }
    }

    fn index(story: Story) -> usize {
        match story {
            Story::Main => 0,
            Story::Footnotes => 1,
            Story::Headers => 2,
            Story::Comments => 4,
            Story::Endnotes => 5,
            Story::TextBoxes => 6,
            Story::HeaderTextBoxes => 7,
        }
    }

    fn range(&self, story: Story) -> Range<u32> {
        let i = Self::index(story);
        self.bounds[i]..self.bounds[i + 1]
    }

    /// A range given relative to the start of a story, cut to the story.
    fn within(&self, story: Story, relative: Range<u32>) -> Range<u32> {
        let whole = self.range(story);
        let start = whole.start.saturating_add(relative.start).min(whole.end);
        let end = whole
            .start
            .saturating_add(relative.end)
            .clamp(start, whole.end);
        start..end
    }
}

/// Footnotes or endnotes: where they are referenced and where their text
/// is.
#[derive(Debug, Default)]
struct Notes<'d> {
    refs: Plc<'d>,
    texts: Plc<'d>,
}

/// The drawings of the main text and of the headers, with the anchors
/// that place their shapes in the text.
#[derive(Debug, Default)]
struct Drawings<'d> {
    store: BlipStore,
    main: Vec<Shape<'d>>,
    header: Vec<Shape<'d>>,
    main_anchors: Plc<'d>,
    header_anchors: Plc<'d>,
    text_boxes: Plc<'d>,
    header_text_boxes: Plc<'d>,
}

impl<'d> Drawings<'d> {
    fn read(fib: &Fib, table: &'d [u8], max: u64) -> Self {
        let slice = |i| fib.slice(table, i).unwrap_or_default();
        let mut drawings = Drawings {
            main_anchors: Plc::new(slice(lcb::PLC_SPA_MOM), 26),
            header_anchors: Plc::new(slice(lcb::PLC_SPA_HDR), 26),
            text_boxes: Plc::new(slice(lcb::PLCFTXBX_TXT), 22),
            header_text_boxes: Plc::new(slice(lcb::PLCF_HDRTXBX_TXT), 22),
            ..Drawings::default()
        };
        let Some(info) = fib.slice(table, lcb::DGG_INFO) else {
            return drawings;
        };
        // The drawing group, then each drawing after a byte that says
        // whether it belongs to the main text (0) or the headers (1).
        let mut records = Records::new(info);
        if let Some(group) = records.next().filter(|r| r.kind == rt::DGG_CONTAINER) {
            drawings.store = BlipStore::from_drawing_group(&group, max);
        }
        let mut rest = records.rest();
        while let Some((&label, after)) = rest.split_first() {
            let mut records = Records::new(after);
            let Some(drawing) = records.next() else {
                break;
            };
            if drawing.kind == rt::DG_CONTAINER {
                let shapes = officeart::shapes(&drawing);
                match label {
                    0 => drawings.main = shapes,
                    _ => drawings.header = shapes,
                }
            }
            rest = records.rest();
        }
        drawings
    }

    /// The shape anchored at a position of the main text or the headers.
    fn anchored(&self, story: Story, cp: u32) -> Option<&Shape<'d>> {
        let (anchors, shapes) = match story {
            Story::Main => (&self.main_anchors, &self.main),
            Story::Headers => (&self.header_anchors, &self.header),
            _ => return None,
        };
        let spa = anchors.data(anchors.find(cp)?)?;
        let id = u32_at(spa, 0)?;
        shapes.iter().find(|s| s.id == id)
    }
}

/// Everything read from the streams that the text walk needs.
#[derive(Debug)]
struct Doc<'d> {
    text: Text<'d>,
    chpx: Bins<'d>,
    papx: Bins<'d>,
    styles: Styles<'d>,
    fonts: Vec<String>,
    stories: Stories,
    footnotes: Notes<'d>,
    endnotes: Notes<'d>,
    comments: Notes<'d>,
    headers: Plc<'d>,
    drawings: Drawings<'d>,
    /// The main stream, where the drawing group keeps its pictures.
    word: &'d [u8],
    /// The `Data` stream, where inline pictures are.
    data: &'d [u8],
}

impl<'d> Doc<'d> {
    fn read(
        fib: &Fib,
        word: &'d [u8],
        table: &'d [u8],
        data: &'d [u8],
        max: u64,
    ) -> Option<Doc<'d>> {
        let text = Text::read(fib.slice(table, lcb::CLX)?, word)?;
        let slice = |i| fib.slice(table, i).unwrap_or_default();
        let stories = Stories::new(&fib.ccp, text.len());
        Some(Doc {
            chpx: Bins::characters(slice(lcb::PLCFBTE_CHPX), word),
            papx: Bins::paragraphs(slice(lcb::PLCFBTE_PAPX), word),
            styles: Styles::read(slice(lcb::STSHF)),
            fonts: styles::fonts(slice(lcb::STTBF_FFN)),
            stories,
            footnotes: Notes {
                refs: Plc::new(slice(lcb::PLCFFND_REF), 2),
                texts: Plc::new(slice(lcb::PLCFFND_TXT), 0),
            },
            endnotes: Notes {
                refs: Plc::new(slice(lcb::PLCFEND_REF), 2),
                texts: Plc::new(slice(lcb::PLCFEND_TXT), 0),
            },
            comments: Notes {
                refs: Plc::new(slice(lcb::PLCFAND_REF), 30),
                texts: Plc::new(slice(lcb::PLCFAND_TXT), 0),
            },
            headers: Plc::new(slice(lcb::PLCFHDD), 0),
            drawings: Drawings::read(fib, table, max),
            text,
            word,
            data,
        })
    }

    /// The properties of the paragraph a character belongs to.
    fn para_props(&self, cp: u32) -> ParaProps {
        let bin = self.text.fc(cp).and_then(|(fc, _)| self.papx.at(fc));
        let (istd, grpprl) = bin.map_or((0, &[][..]), |b| (b.istd, b.grpprl));
        ParaProps::read(istd, [grpprl, self.text.piece_sprms(cp)].into_iter())
    }

    /// Whether a page or section break ends its paragraph.
    fn ends_paragraph(&self, cp: u32) -> bool {
        self.text
            .fc(cp)
            .is_none_or(|(fc, width)| self.papx.ends_at(fc, width))
    }

    /// The visible text of a range on one line, for comment anchors: field
    /// instructions and special characters left out.
    fn plain(&self, range: Range<u32>) -> String {
        let mut units = Vec::new();
        let mut fields: Vec<bool> = Vec::new();
        for cp in range {
            match self.text.unit(cp) {
                0x13 => fields.push(false),
                0x14 => {
                    if let Some(f) = fields.last_mut() {
                        *f = true;
                    }
                }
                0x15 => {
                    fields.pop();
                }
                _ if fields.last() == Some(&false) => {}
                0x07 | 0x09 | 0x0B..=0x0D => units.push(u16::from(b' ')),
                u if u < 0x20 => {}
                u => units.push(u),
            }
        }
        String::from_utf16_lossy(&units)
    }
}

/// Headers and footers: after six separator stories, six per section
/// (even and odd header, even and odd footer, first-page header and
/// footer). Each distinct text is kept once.
fn headers_footers(walker: &mut Walker<'_, '_, '_>, doc: &Doc<'_>) -> Option<Section> {
    let mut section = Section::new(SectionKind::HeadersFooters);
    let mut texts: Vec<String> = Vec::new();
    for i in 6..doc.headers.len() {
        let Some(range) = doc.headers.range(i) else {
            break;
        };
        let range = doc.stories.within(Story::Headers, range);
        if range.is_empty() {
            continue;
        }
        let (kind, title) = match (i - 6) % 6 {
            0 => (GroupKind::Header, Some("even pages")),
            1 => (GroupKind::Header, None),
            2 => (GroupKind::Footer, Some("even pages")),
            3 => (GroupKind::Footer, None),
            4 => (GroupKind::Header, Some("first page")),
            _ => (GroupKind::Footer, Some("first page")),
        };
        walker.area = if kind == GroupKind::Header {
            "header"
        } else {
            "footer"
        }
        .into();
        let blocks = walker.story(Story::Headers, range);
        if blocks.iter().all(Block::is_blank) {
            continue;
        }
        let text = blocks_plain_text(&blocks).trim().to_string();
        if !text.is_empty() {
            if texts.contains(&text) {
                continue;
            }
            texts.push(text);
        }
        section.blocks.push(Block::Group(Group {
            kind,
            title: title.map(str::to_string),
            blocks,
        }));
    }
    walker.area = "body".into();
    (!section.blocks.is_empty()).then_some(section)
}

/// Footnotes or endnotes, labelled in the order they are referenced.
fn notes(walker: &mut Walker<'_, '_, '_>, doc: &Doc<'_>, kind: NoteKind) -> Option<Section> {
    let (notes, story, area, section_kind) = match kind {
        NoteKind::Endnote => (
            &doc.endnotes,
            Story::Endnotes,
            "endnotes",
            SectionKind::Endnotes,
        ),
        _ => (
            &doc.footnotes,
            Story::Footnotes,
            "footnotes",
            SectionKind::Footnotes,
        ),
    };
    walker.area = area.into();
    let mut section = Section::new(section_kind);
    for i in 0..notes.refs.len() {
        let Some(range) = notes.texts.range(i) else {
            break;
        };
        let blocks = walker.story(story, doc.stories.within(story, range));
        if blocks.iter().all(Block::is_blank) {
            continue;
        }
        section.blocks.push(Block::Note(Note {
            kind,
            label: note_label(kind, i),
            author: None,
            date: None,
            anchor: None,
            blocks,
        }));
    }
    walker.area = "body".into();
    (!section.blocks.is_empty()).then_some(section)
}

/// The label of the `index`-th note of a kind: `1`, `e1`, `c1`.
fn note_label(kind: NoteKind, index: usize) -> String {
    let prefix = match kind {
        NoteKind::Footnote => "",
        NoteKind::Endnote => "e",
        NoteKind::Comment => "c",
    };
    format!("{prefix}{}", index + 1)
}

/// What the table stream says about comments besides their text.
#[derive(Debug, Default)]
struct Comments<'t> {
    /// Author names.
    owners: Vec<String>,
    /// Dates and reply links (Word 2002 and later): 18 bytes per comment.
    extra: &'t [u8],
    /// Bookmarks marking the commented text: tags, starts and ends.
    tags: Vec<u32>,
    starts: Plc<'t>,
    ends: Plc<'t>,
}

/// Size of an `ATRDPost10`.
const ATRD_EXTRA_SIZE: usize = 18;

impl<'t> Comments<'t> {
    fn read(fib: &Fib, table: &'t [u8]) -> Self {
        let slice = |i| fib.slice(table, i).unwrap_or_default();
        let mut owners = Vec::new();
        let mut r = Cursor::new(slice(lcb::GRP_XST_ATN_OWNERS));
        while let Some(len) = r.u16() {
            let Some(name) = r.take(usize::from(len) * 2) else {
                break;
            };
            owners.push(utf16le(name));
        }
        Comments {
            owners,
            extra: slice(lcb::ATRD_EXTRA),
            tags: bookmark_tags(slice(lcb::STTBF_ATN_BKMK)),
            starts: Plc::new(slice(lcb::PLCF_ATN_BKF), 4),
            ends: Plc::new(slice(lcb::PLCF_ATN_BKL), 0),
        }
    }

    fn extra(&self, i: usize) -> Option<&'t [u8]> {
        let start = i.checked_mul(ATRD_EXTRA_SIZE)?;
        self.extra.get(start..start + ATRD_EXTRA_SIZE)
    }

    /// The range of text the comment with this bookmark tag is attached to.
    fn anchor(&self, tag: u32) -> Option<Range<u32>> {
        let k = self.tags.iter().position(|&t| t == tag)?;
        let start = self.starts.cp(k)?;
        let end_index = u16_at(self.starts.data(k)?, 0)?;
        let end = self.ends.cp(usize::from(end_index))?;
        (start < end).then_some(start..end)
    }
}

/// The tags of the comment bookmarks (`SttbfAtnBkmk`): an extended string
/// table whose strings are empty and whose extra data holds the tags.
fn bookmark_tags(sttb: &[u8]) -> Vec<u32> {
    let mut r = Cursor::new(sttb);
    let (Some(0xFFFF), Some(count), Some(extra)) = (r.u16(), r.u16(), r.u16()) else {
        return Vec::new();
    };
    let mut tags = Vec::new();
    for _ in 0..count {
        let Some(len) = r.u16() else {
            break;
        };
        let (Some(_), Some(data)) = (r.take(usize::from(len) * 2), r.take(usize::from(extra)))
        else {
            break;
        };
        tags.push(u32_at(data, 2).unwrap_or(u32::MAX));
    }
    tags
}

/// Review comments with their authors, dates, replies and the text they
/// are attached to.
fn comments_section(
    walker: &mut Walker<'_, '_, '_>,
    doc: &Doc<'_>,
    comments: &Comments<'_>,
) -> Option<Section> {
    walker.area = "comments".into();
    let mut section = Section::new(SectionKind::Comments);
    let refs = &doc.comments.refs;
    for i in 0..refs.len() {
        let (Some(range), Some(atrd)) = (doc.comments.texts.range(i), refs.data(i)) else {
            break;
        };
        let mut blocks = walker.story(Story::Comments, doc.stories.within(Story::Comments, range));
        let extra = comments.extra(i);
        let parent = extra
            .and_then(|e| i32_at(e, 10))
            .filter(|&delta| delta != 0)
            .and_then(|delta| usize::try_from(i as i64 + i64::from(delta)).ok())
            .filter(|&p| p < refs.len());
        if let Some(parent) = parent {
            prepend(
                &mut blocks,
                &format!("(reply to {}) ", note_label(NoteKind::Comment, parent)),
            );
        }
        let author = u16_at(atrd, 20)
            .and_then(|ibst| comments.owners.get(usize::from(ibst)))
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty());
        let anchor = u32_at(atrd, 26)
            .and_then(|tag| comments.anchor(tag))
            .and_then(|range| anchor_text(&doc.plain(range)));
        section.blocks.push(Block::Note(Note {
            kind: NoteKind::Comment,
            label: note_label(NoteKind::Comment, i),
            author,
            date: extra.and_then(|e| u32_at(e, 0)).and_then(dttm),
            anchor,
            blocks,
        }));
    }
    walker.area = "body".into();
    (!section.blocks.is_empty()).then_some(section)
}

/// A packed date and time (`DTTM`) as `YYYY-MM-DD HH:MM`.
fn dttm(value: u32) -> Option<String> {
    let minute = value & 0x3F;
    let hour = (value >> 6) & 0x1F;
    let day = (value >> 11) & 0x1F;
    let month = (value >> 16) & 0xF;
    let year = 1900 + ((value >> 20) & 0x1FF);
    let valid = (1..=12).contains(&month) && (1..=31).contains(&day) && hour < 24 && minute < 60;
    valid.then(|| format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_unpack() {
        // 2009-03-21 14:05
        let value = 5 | 14 << 6 | 21 << 11 | 3 << 16 | 109 << 20;
        assert_eq!(dttm(value).as_deref(), Some("2009-03-21 14:05"));
        assert_eq!(dttm(0), None);
    }

    #[test]
    fn stories_follow_each_other() {
        let ccp = StoryLengths {
            text: 10,
            footnotes: 4,
            headers: 6,
            ..StoryLengths::default()
        };
        let stories = Stories::new(&ccp, 100);
        assert_eq!(stories.range(Story::Headers), 14..20);
        assert_eq!(stories.within(Story::Headers, 2..9), 16..20);
        assert_eq!(stories.range(Story::Comments), 20..20);
    }

    #[test]
    fn comment_bookmarks_have_tags() {
        let mut sttb = vec![0xFF, 0xFF, 1, 0, 10, 0, 0, 0];
        sttb.extend([0, 0, 7, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(bookmark_tags(&sttb), [7]);
    }
}
