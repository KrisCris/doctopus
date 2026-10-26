//! PowerPoint 97-2003 presentations (`.ppt`, `.pps`, `.pot`).
//!
//! The `PowerPoint Document` stream is a sequence of records that grows
//! with every save. The `Current User` stream points to the last save, and
//! the persist directories of the saves map object ids to the offsets of
//! their latest versions.
//!
//! Slides are read in presentation order. A slide's drawing lists its
//! shapes in z-order: placeholders and text boxes (their text kept in the
//! shape or, in older files, in the slide list), tables, pictures from the
//! `Pictures` stream, and embedded OLE objects. Speaker notes and review
//! comments are attached to their slide. Masters are only read for the
//! bullet defaults of their text styles.

use std::collections::{HashMap, HashSet};

use doctopus_core::{
    Block, Document, EmbedRef, Error, Group, GroupKind, ImageRef, Inline, Input, Link, ListItem,
    Note, NoteKind, Part, PartKind, Result, Section, SectionKind, Style, Table, TableCell,
    TableRow, push_text,
};

use super::context::Context;
use super::officeart::{self, BlipStore, Record, Records, Shape};
use crate::binary::{Cursor, u32_at};
use crate::blocks::{link_shape, push_text_box};
use crate::ole::object;
use crate::ole::text::utf16le;

/// Record types.
mod rt {
    pub(super) const DOCUMENT: u16 = 0x03E8;
    pub(super) const SLIDE: u16 = 0x03EE;
    pub(super) const SLIDE_ATOM: u16 = 0x03EF;
    pub(super) const NOTES: u16 = 0x03F0;
    pub(super) const SLIDE_PERSIST_ATOM: u16 = 0x03F3;
    pub(super) const MAIN_MASTER: u16 = 0x03F8;
    pub(super) const SLIDE_SHOW_SLIDE_INFO_ATOM: u16 = 0x03F9;
    pub(super) const VBA_INFO: u16 = 0x03FF;
    pub(super) const VBA_INFO_ATOM: u16 = 0x0400;
    pub(super) const EX_OBJ_LIST: u16 = 0x0409;
    pub(super) const DRAWING_GROUP: u16 = 0x040B;
    pub(super) const DRAWING: u16 = 0x040C;
    pub(super) const DOC_INFO_LIST: u16 = 0x07D0;
    pub(super) const ROUND_TRIP_HF_PLACEHOLDER: u16 = 0x0420;
    pub(super) const EX_OBJ_REF_ATOM: u16 = 0x0BC1;
    pub(super) const PLACEHOLDER_ATOM: u16 = 0x0BC3;
    pub(super) const OUTLINE_TEXT_REF_ATOM: u16 = 0x0F9E;
    pub(super) const TEXT_HEADER_ATOM: u16 = 0x0F9F;
    pub(super) const TEXT_CHARS_ATOM: u16 = 0x0FA0;
    pub(super) const STYLE_TEXT_PROP_ATOM: u16 = 0x0FA1;
    pub(super) const TEXT_MASTER_STYLE_ATOM: u16 = 0x0FA3;
    pub(super) const TEXT_BYTES_ATOM: u16 = 0x0FA8;
    pub(super) const CSTRING: u16 = 0x0FBA;
    pub(super) const EX_OLE_OBJ_ATOM: u16 = 0x0FC3;
    pub(super) const EX_OLE_EMBED: u16 = 0x0FCC;
    pub(super) const EX_HYPERLINK_ATOM: u16 = 0x0FD3;
    pub(super) const EX_HYPERLINK: u16 = 0x0FD7;
    pub(super) const TEXT_INTERACTIVE_INFO_ATOM: u16 = 0x0FDF;
    pub(super) const SLIDE_LIST_WITH_TEXT: u16 = 0x0FF0;
    pub(super) const INTERACTIVE_INFO: u16 = 0x0FF2;
    pub(super) const INTERACTIVE_INFO_ATOM: u16 = 0x0FF3;
    pub(super) const USER_EDIT_ATOM: u16 = 0x0FF5;
    pub(super) const CURRENT_USER_ATOM: u16 = 0x0FF6;
    pub(super) const EX_MEDIA_ATOM: u16 = 0x1004;
    pub(super) const EX_OLE_OBJ_STG: u16 = 0x1011;
    pub(super) const PROG_TAGS: u16 = 0x1388;
    pub(super) const PROG_BINARY_TAG: u16 = 0x138A;
    pub(super) const BINARY_TAG_DATA_BLOB: u16 = 0x138B;
    pub(super) const PERSIST_DIRECTORY_ATOM: u16 = 0x1772;
    pub(super) const COMMENT10: u16 = 0x2EE0;
    pub(super) const COMMENT10_ATOM: u16 = 0x2EE1;
    pub(super) const CRYPT_SESSION: u16 = 0x2F14;
}

/// What a text body is for (`TextHeaderAtom.textType`).
mod text_type {
    pub(super) const TITLE: u32 = 0;
    pub(super) const BODY: u32 = 1;
    pub(super) const OTHER: u32 = 4;
    pub(super) const CENTER_BODY: u32 = 5;
    pub(super) const CENTER_TITLE: u32 = 6;
    pub(super) const HALF_BODY: u32 = 7;
    pub(super) const QUARTER_BODY: u32 = 8;
}

/// Placeholder kinds (`OEPlaceholderAtom.placementId`).
mod placeholder {
    pub(super) const MASTER_DATE: u8 = 0x07;
    pub(super) const MASTER_SLIDE_NUMBER: u8 = 0x08;
    pub(super) const MASTER_FOOTER: u8 = 0x09;
    pub(super) const MASTER_HEADER: u8 = 0x0A;
    pub(super) const NOTES_SLIDE_IMAGE: u8 = 0x0B;
    pub(super) const TITLE: u8 = 0x0D;
    pub(super) const CENTER_TITLE: u8 = 0x0F;
    pub(super) const VERTICAL_TITLE: u8 = 0x11;
}

/// `CurrentUserAtom.headerToken` of an encrypted file.
const ENCRYPTED_TOKEN: u32 = 0xF3D1_C4DF;
/// `InteractiveInfoAtom.action` that follows a hyperlink.
const ACTION_HYPERLINK: u8 = 4;
/// `msosptLine`: the borders of table cells.
const SHAPE_LINE: u16 = 20;
/// How far apart (in master units, 1/576 inch) table cell edges may be and
/// still line up.
const EDGE_TOLERANCE: i32 = 8;
/// Saves followed at most when walking the edit chain.
const MAX_EDITS: usize = 10_000;
/// Indent levels of PowerPoint text.
const LEVELS: usize = 5;

/// Read a `.ppt` presentation.
pub(crate) fn read(input: &Input<'_>) -> Result<Document> {
    let mut cx = Context::open(input)?;
    let format = input.format.name;
    if cx.cfb.is_stream("/EncryptedSummary") {
        return Err(Error::encrypted(format));
    }
    let current_user = cx.stream("/Current User");
    let is_pp95 = !cx.cfb.is_stream("/PowerPoint Document") && cx.cfb.is_stream("/PP40")
        || current_user.as_deref().is_some_and(is_pp95_current_user);
    if is_pp95 {
        return Err(Error::Unsupported {
            detected: Some(
                "PowerPoint 95 presentation; save it in a newer format to read it".into(),
            ),
        });
    }
    let stream = cx.required_stream("/PowerPoint Document")?;
    let persist = Persist::read(&stream, current_user.as_deref());
    if persist.encrypted {
        return Err(Error::encrypted(format));
    }
    let document = match persist.record(&stream, persist.document) {
        Some(record) if record.kind == rt::DOCUMENT && !persist.damaged => record,
        found => {
            cx.doc
                .warn("the save history of the presentation is damaged; slides may be incomplete");
            found
                .filter(|r| r.kind == rt::DOCUMENT)
                .or_else(|| {
                    Records::new(&stream)
                        .filter(|r| r.kind == rt::DOCUMENT)
                        .last()
                })
                .ok_or_else(|| Error::malformed(format, "the document record is missing"))?
        }
    };
    let pictures = cx.stream("/Pictures");
    let pres = Presentation::read(&mut cx, &stream, persist, &document, pictures);
    let mut comments = 0;
    for (i, entry) in pres.slides.iter().enumerate() {
        let number = u32::try_from(i + 1).unwrap_or(u32::MAX);
        let section = pres.slide(&mut cx, entry, number, &mut comments);
        cx.doc.sections.push(section);
    }
    Ok(cx.doc)
}

// ----- the edit chain ----------------------------------------------------------

/// Where the latest version of each persistent object is.
#[derive(Debug, Default)]
struct Persist {
    /// Stream offset by persist id.
    offsets: HashMap<u32, usize>,
    /// Persist id of the document record.
    document: u32,
    encrypted: bool,
    /// The edit chain is broken; what was found may be out of date.
    damaged: bool,
}

/// One save (`UserEditAtom`).
#[derive(Debug)]
struct UserEdit {
    /// Offset of the previous save's edit, 0 for the first save.
    last_edit: usize,
    directory: usize,
    document: u32,
    crypt_session: Option<u32>,
}

impl Persist {
    /// Follow the edit chain from the last save back to the first; the
    /// newest entry for an object wins.
    fn read(stream: &[u8], current_user: Option<&[u8]>) -> Persist {
        let mut persist = Persist::default();
        let mut next = match current_user.and_then(current_edit) {
            Some((ENCRYPTED_TOKEN, _)) => {
                persist.encrypted = true;
                return persist;
            }
            Some((_, offset)) if UserEdit::read(stream, offset).is_some() => Some(offset),
            // Without a usable `Current User` stream, the last edit in the
            // stream is the current one.
            _ => last_user_edit(stream),
        };
        if next.is_none() {
            persist.damaged = true;
        }
        let mut seen = HashSet::new();
        let mut crypt_session = None;
        let mut first = true;
        while let Some(offset) = next {
            if !seen.insert(offset) || seen.len() > MAX_EDITS {
                persist.damaged = true;
                break;
            }
            let Some(edit) = UserEdit::read(stream, offset) else {
                persist.damaged = true;
                break;
            };
            if first {
                persist.document = edit.document;
                crypt_session = edit.crypt_session;
                first = false;
            }
            if read_directory(stream, edit.directory, &mut persist.offsets).is_none() {
                persist.damaged = true;
            }
            next = (edit.last_edit != 0).then_some(edit.last_edit);
        }
        if let Some(id) = crypt_session
            && persist
                .record(stream, id)
                .is_some_and(|r| r.kind == rt::CRYPT_SESSION)
        {
            persist.encrypted = true;
        }
        persist
    }

    /// The record of a persistent object.
    fn record<'s>(&self, stream: &'s [u8], id: u32) -> Option<Record<'s>> {
        let offset = *self.offsets.get(&id)?;
        Records::new(stream.get(offset..)?).next()
    }
}

impl UserEdit {
    fn read(stream: &[u8], offset: usize) -> Option<UserEdit> {
        let record = Records::new(stream.get(offset..)?)
            .next()
            .filter(|r| r.kind == rt::USER_EDIT_ATOM)?;
        let mut r = Cursor::new(record.data);
        r.skip(4 + 2 + 1 + 1)?;
        let last_edit = r.u32()? as usize;
        let directory = r.u32()? as usize;
        let document = r.u32()?;
        r.skip(4 + 2 + 2)?;
        Some(UserEdit {
            last_edit,
            directory,
            document,
            crypt_session: r.u32(),
        })
    }
}

/// The header token and current edit offset of the `Current User` stream.
fn current_edit(data: &[u8]) -> Option<(u32, usize)> {
    let record = Records::new(data)
        .next()
        .filter(|r| r.kind == rt::CURRENT_USER_ATOM)?;
    let mut r = Cursor::new(record.data);
    r.skip(4)?;
    let token = r.u32()?;
    Some((token, r.u32()? as usize))
}

/// PowerPoint 95 writes a shorter `Current User` stream: a 4-byte size,
/// then that many bytes.
fn is_pp95_current_user(data: &[u8]) -> bool {
    data.len() < 28 && u32_at(data, 0).is_some_and(|size| size as usize + 4 == data.len())
}

/// The offset of the last top-level edit record.
fn last_user_edit(stream: &[u8]) -> Option<usize> {
    let mut records = Records::new(stream);
    let mut last = None;
    loop {
        let offset = stream.len() - records.rest().len();
        let Some(record) = records.next() else {
            return last;
        };
        if record.kind == rt::USER_EDIT_ATOM {
            last = Some(offset);
        }
    }
}

/// Add the entries of a persist directory that are not known yet.
fn read_directory(stream: &[u8], offset: usize, offsets: &mut HashMap<u32, usize>) -> Option<()> {
    let record = Records::new(stream.get(offset..)?)
        .next()
        .filter(|r| r.kind == rt::PERSIST_DIRECTORY_ATOM)?;
    let mut r = Cursor::new(record.data);
    while r.remaining() >= 4 {
        let entry = r.u32()?;
        let first = entry & 0xF_FFFF;
        for i in 0..entry >> 20 {
            let offset = r.u32()? as usize;
            offsets.entry(first + i).or_insert(offset);
        }
    }
    Some(())
}

// ----- the presentation --------------------------------------------------------

/// An entry of a slide, master or notes list.
#[derive(Debug)]
struct ListEntry {
    persist: u32,
    /// Slide, master or notes id that other records refer to.
    id: u32,
    /// Text bodies kept in the list (`OutlineTextRefAtom` targets).
    texts: Vec<TextBody>,
}

/// An external object (`ExObjList` entry) a shape can show.
#[derive(Debug)]
enum ExObject {
    /// An embedded OLE object, stored in an `ExOleObjStg` record.
    Embed {
        persist: u32,
        prog_id: Option<String>,
    },
    /// A linked sound or movie file.
    Media { path: String },
}

/// What the document record says about the whole presentation.
struct Presentation<'s> {
    stream: &'s [u8],
    persist: Persist,
    store: BlipStore,
    /// The `Pictures` stream, where the picture store keeps its pictures.
    pictures: Option<Vec<u8>>,
    hyperlinks: HashMap<u32, String>,
    objects: HashMap<u32, ExObject>,
    slides: Vec<ListEntry>,
    notes: Vec<ListEntry>,
    /// Bullet defaults by master id.
    bullets: HashMap<u32, MasterBullets>,
}

impl<'s> Presentation<'s> {
    fn read(
        cx: &mut Context<'_>,
        stream: &'s [u8],
        persist: Persist,
        document: &Record<'_>,
        pictures: Option<Vec<u8>>,
    ) -> Self {
        let mut pres = Presentation {
            stream,
            persist,
            store: BlipStore::default(),
            pictures,
            hyperlinks: HashMap::new(),
            objects: HashMap::new(),
            slides: Vec::new(),
            notes: Vec::new(),
            bullets: HashMap::new(),
        };
        let mut masters = Vec::new();
        for record in document.children() {
            match record.kind {
                rt::EX_OBJ_LIST => pres.read_objects(&record),
                rt::DRAWING_GROUP => {
                    if let Some(dgg) = record.child(officeart::rt::DGG_CONTAINER) {
                        pres.store = BlipStore::from_drawing_group(&dgg, cx.options.max_part_size);
                    }
                }
                rt::SLIDE_LIST_WITH_TEXT => match record.instance {
                    0 => pres.slides = list_entries(&record),
                    1 => masters = list_entries(&record),
                    2 => pres.notes = list_entries(&record),
                    _ => {}
                },
                rt::DOC_INFO_LIST => {
                    let has_macros = record
                        .child(rt::VBA_INFO)
                        .and_then(|info| info.child(rt::VBA_INFO_ATOM))
                        .and_then(|atom| u32_at(atom.data, 4))
                        == Some(1);
                    if has_macros {
                        cx.doc.metadata.set("macros", "yes (VBA project)");
                    }
                }
                _ => {}
            }
        }
        pres.read_masters(&masters);
        pres
    }

    fn record(&self, id: u32) -> Option<Record<'s>> {
        self.persist.record(self.stream, id)
    }

    /// Hyperlinks, embedded objects and media of the external object list.
    fn read_objects(&mut self, list: &Record<'_>) {
        for item in list.children() {
            match item.kind {
                rt::EX_HYPERLINK => {
                    let id = item
                        .child(rt::EX_HYPERLINK_ATOM)
                        .and_then(|atom| u32_at(atom.data, 0));
                    let url = hyperlink_url(cstring(&item, 1), cstring(&item, 3));
                    if let (Some(id), Some(url)) = (id, url) {
                        self.hyperlinks.insert(id, url);
                    }
                }
                rt::EX_OLE_EMBED => {
                    let Some(atom) = item.child(rt::EX_OLE_OBJ_ATOM) else {
                        continue;
                    };
                    if let (Some(id), Some(persist)) = (u32_at(atom.data, 8), u32_at(atom.data, 16))
                    {
                        let prog_id = cstring(&item, 2);
                        self.objects
                            .insert(id, ExObject::Embed { persist, prog_id });
                    }
                }
                _ => {
                    // Sounds hold their media atom directly, movies in a
                    // video container.
                    let media = std::iter::once(item)
                        .chain(item.children().filter(Record::is_container))
                        .find(|r| r.child(rt::EX_MEDIA_ATOM).is_some());
                    if let Some(media) = media
                        && let Some(id) = media
                            .child(rt::EX_MEDIA_ATOM)
                            .and_then(|a| u32_at(a.data, 0))
                        && let Some(path) = cstring(&media, 0)
                    {
                        self.objects.insert(id, ExObject::Media { path });
                    }
                }
            }
        }
    }

    /// Bullet defaults of each master. Title masters take them from the
    /// main master they are based on.
    fn read_masters(&mut self, masters: &[ListEntry]) {
        let mut based_on = Vec::new();
        for entry in masters {
            let Some(record) = self.record(entry.persist) else {
                continue;
            };
            match record.kind {
                rt::MAIN_MASTER => {
                    self.bullets.insert(entry.id, MasterBullets::read(&record));
                }
                rt::SLIDE => {
                    if let Some(main) = record
                        .child(rt::SLIDE_ATOM)
                        .and_then(|a| u32_at(a.data, 12))
                    {
                        based_on.push((entry.id, main));
                    }
                }
                _ => {}
            }
        }
        for (id, main) in based_on {
            if let Some(bullets) = self.bullets.get(&main).copied() {
                self.bullets.insert(id, bullets);
            }
        }
    }

    fn slide(
        &self,
        cx: &mut Context<'_>,
        entry: &ListEntry,
        number: u32,
        comments: &mut u32,
    ) -> Section {
        let mut section = Section::new(SectionKind::Slide).with_number(number);
        let location = format!("slide {number}");
        let record = self.record(entry.persist).filter(|r| r.kind == rt::SLIDE);
        if record.is_none() {
            cx.doc.warn(format!("slide {number} is missing"));
        }
        let atom = record.and_then(|r| r.child(rt::SLIDE_ATOM));
        let bullets = atom
            .and_then(|a| u32_at(a.data, 12))
            .and_then(|id| self.bullets.get(&id))
            .copied()
            .unwrap_or_default();
        let mut page = Page {
            pres: self,
            cx,
            location: &location,
            outline: &entry.texts,
            bullets,
            title: None,
        };
        let drawing = record.and_then(|r| drawing(&r));
        section.blocks = match drawing {
            Some(dg) => page.shapes(&dg),
            None => page.outline_blocks(),
        };
        if let Some(title) = page.title.take() {
            section = section.with_name(title);
        }
        section.hidden = record
            .and_then(|r| r.child(rt::SLIDE_SHOW_SLIDE_INFO_ATOM))
            .and_then(|a| a.data.get(10).copied())
            .is_some_and(|flags| flags & 0x4 != 0);
        if let Some(notes_id) = atom.and_then(|a| u32_at(a.data, 16)).filter(|&id| id != 0)
            && let Some(notes) = self.notes(cx, notes_id, &location)
        {
            section.blocks.push(notes);
        }
        if let Some(record) = record {
            section.blocks.extend(slide_comments(&record, comments));
        }
        section
    }

    /// The speaker notes of a slide.
    fn notes(&self, cx: &mut Context<'_>, notes_id: u32, location: &str) -> Option<Block> {
        let entry = self.notes.iter().find(|e| e.id == notes_id)?;
        let record = self.record(entry.persist).filter(|r| r.kind == rt::NOTES);
        let location = format!("{location} notes");
        let mut page = Page {
            pres: self,
            cx,
            location: &location,
            outline: &entry.texts,
            bullets: MasterBullets::default(),
            title: None,
        };
        let blocks = match record.as_ref().and_then(drawing) {
            Some(dg) => page.shapes(&dg),
            None => page.outline_blocks(),
        };
        let has_text = blocks.iter().any(|b| !b.is_blank());
        has_text.then(|| {
            Block::Group(Group {
                kind: GroupKind::Notes,
                title: Some("Notes".into()),
                blocks,
            })
        })
    }
}

/// The drawing (`OfficeArtDgContainer`) of a slide or notes page.
fn drawing<'s>(record: &Record<'s>) -> Option<Record<'s>> {
    record
        .child(rt::DRAWING)?
        .child(officeart::rt::DG_CONTAINER)
}

/// The entries of a slide list, each with the text records that follow it.
fn list_entries(list: &Record<'_>) -> Vec<ListEntry> {
    let mut entries: Vec<(ListEntry, Vec<Record<'_>>)> = Vec::new();
    for record in list.children() {
        if record.kind == rt::SLIDE_PERSIST_ATOM {
            let (Some(persist), Some(id)) = (u32_at(record.data, 0), u32_at(record.data, 12))
            else {
                continue;
            };
            let entry = ListEntry {
                persist,
                id,
                texts: Vec::new(),
            };
            entries.push((entry, Vec::new()));
        } else if let Some((_, records)) = entries.last_mut() {
            records.push(record);
        }
    }
    entries
        .into_iter()
        .map(|(mut entry, records)| {
            entry.texts = text_bodies(records);
            entry
        })
        .collect()
}

/// A CString child of a container, by instance.
fn cstring(container: &Record<'_>, instance: u16) -> Option<String> {
    container
        .children()
        .find(|r| r.kind == rt::CSTRING && r.instance == instance)
        .map(|r| utf16le(r.data).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The URL of a hyperlink from its target and location. Links to slides
/// have locations like `258,3,Title` (slide id, number, title).
fn hyperlink_url(target: Option<String>, location: Option<String>) -> Option<String> {
    let slide = location.as_deref().and_then(|l| {
        let mut parts = l.splitn(3, ',');
        let (Some(id), Some(number)) = (parts.next(), parts.next()) else {
            return None;
        };
        id.parse::<u32>().ok()?;
        number.parse::<u32>().ok().map(|n| format!("#slide-{n}"))
    });
    match (target, location) {
        (Some(t), Some(l)) => Some(format!("{t}#{l}")),
        (Some(t), None) => Some(t),
        (None, Some(l)) => Some(slide.unwrap_or_else(|| format!("#{l}"))),
        (None, None) => None,
    }
}

/// The hyperlink an `InteractiveInfoAtom` follows, if that is its action.
fn hyperlink_ref(atom: &[u8]) -> Option<u32> {
    (atom.get(8) == Some(&ACTION_HYPERLINK))
        .then(|| u32_at(atom, 4))
        .flatten()
}

// ----- slides and notes pages ----------------------------------------------------

/// The records of a shape's client data that matter here.
#[derive(Debug, Default)]
struct ClientData {
    placeholder: Option<u8>,
    /// External object shown by the shape.
    object: Option<u32>,
    /// Hyperlink followed when the shape is clicked.
    link: Option<u32>,
}

impl ClientData {
    fn read(data: Option<&[u8]>) -> ClientData {
        let mut out = ClientData::default();
        for record in Records::new(data.unwrap_or_default()) {
            match record.kind {
                rt::PLACEHOLDER_ATOM => {
                    out.placeholder = record.data.get(4).copied().filter(|&p| p != 0);
                }
                // PowerPoint 2007 and later save slide footers as text boxes
                // that remember which placeholder they were.
                rt::ROUND_TRIP_HF_PLACEHOLDER if out.placeholder.is_none() => {
                    out.placeholder = record.data.first().copied().filter(|&p| p != 0);
                }
                rt::EX_OBJ_REF_ATOM => out.object = u32_at(record.data, 0),
                rt::INTERACTIVE_INFO if record.instance == 0 => {
                    out.link = record
                        .child(rt::INTERACTIVE_INFO_ATOM)
                        .and_then(|a| hyperlink_ref(a.data));
                }
                _ => {}
            }
        }
        out
    }
}

/// Reads the shapes of one slide or notes page.
struct Page<'r, 's, 'x> {
    pres: &'r Presentation<'s>,
    cx: &'r mut Context<'x>,
    location: &'r str,
    /// Text bodies of the page kept in the slide list.
    outline: &'r [TextBody],
    bullets: MasterBullets,
    /// The first title, which names the slide.
    title: Option<String>,
}

impl Page<'_, '_, '_> {
    fn shapes(&mut self, drawing: &Record<'_>) -> Vec<Block> {
        let mut out = Vec::new();
        for shape in officeart::shapes(drawing) {
            self.shape(&shape, &mut out);
        }
        out
    }

    /// The text of the slide list alone, for pages whose drawing is lost.
    fn outline_blocks(&mut self) -> Vec<Block> {
        let mut out = Vec::new();
        for body in self.outline {
            self.text(body, None, &mut out);
        }
        out
    }

    fn shape(&mut self, shape: &Shape<'_>, out: &mut Vec<Block>) {
        if shape.is_deleted() {
            return;
        }
        if !shape.children.is_empty() {
            if shape.props.table
                && let Some(table) = self.table(shape)
            {
                out.push(Block::Table(table));
                return;
            }
            for child in &shape.children {
                self.shape(child, out);
            }
            return;
        }
        let data = ClientData::read(shape.client_data);
        if matches!(
            data.placeholder,
            Some(
                placeholder::MASTER_DATE
                    | placeholder::MASTER_SLIDE_NUMBER
                    | placeholder::MASTER_FOOTER
                    | placeholder::MASTER_HEADER
                    | placeholder::NOTES_SLIDE_IMAGE
            )
        ) {
            return;
        }
        let link = data
            .link
            .and_then(|id| self.pres.hyperlinks.get(&id))
            .cloned();
        let mut blocks = Vec::new();
        let object = data.object.and_then(|id| self.object(id));
        if let Some(object) = object {
            out.push(object);
        } else if let Some(index) = shape.props.blip
            && let Some(image) = self.picture(index, shape)
        {
            out.push(image);
        }
        if let Some(body) = self.shape_text(shape) {
            self.text(&body, data.placeholder, &mut blocks);
        } else if let Some(text) = &shape.props.word_art {
            blocks.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(Block::text),
            );
        }
        if let Some(url) = link {
            link_shape(&mut blocks, url);
        }
        if data.placeholder.is_some() {
            out.append(&mut blocks);
        } else {
            push_text_box(out, blocks);
        }
    }

    /// Add the blocks of a text body; the first title names the page
    /// instead.
    fn text(&mut self, body: &TextBody, placeholder: Option<u8>, out: &mut Vec<Block>) {
        let is_title = matches!(
            placeholder,
            Some(placeholder::TITLE | placeholder::CENTER_TITLE | placeholder::VERTICAL_TITLE)
        ) || matches!(body.text_type, text_type::TITLE | text_type::CENTER_TITLE);
        if is_title && self.title.is_none() {
            let title = body.plain();
            if !title.is_empty() {
                self.title = Some(title);
            }
            return;
        }
        let defaults = self.bullets.defaults(body.text_type);
        out.extend(body.blocks(defaults, &self.pres.hyperlinks));
    }

    /// The text of a shape: its own, or the slide-list text it refers to.
    fn shape_text(&self, shape: &Shape<'_>) -> Option<TextBody> {
        let records: Vec<Record<'_>> = Records::new(shape.client_textbox?).collect();
        let reference = records.iter().find(|r| r.kind == rt::OUTLINE_TEXT_REF_ATOM);
        if let Some(reference) = reference {
            let index = u32_at(reference.data, 0)? as usize;
            let mut body = self.outline.get(index)?.clone();
            body.links.extend(text_links(&records));
            return Some(body);
        }
        text_bodies(records).into_iter().next()
    }

    /// An embedded object, or a linked media file.
    fn object(&mut self, id: u32) -> Option<Block> {
        match self.pres.objects.get(&id)? {
            ExObject::Embed { persist, prog_id } => {
                let origin = format!("ole:{persist}");
                if let Some(part) = self.cx.doc.part_by_origin(&origin) {
                    let part = part.id.clone();
                    return Some(Block::Embed(EmbedRef { part }));
                }
                let record = self
                    .pres
                    .record(*persist)
                    .filter(|r| r.kind == rt::EX_OLE_OBJ_STG);
                let data = record.and_then(|r| ole_storage(&r, self.cx.options.max_part_size));
                let Some(data) = data else {
                    self.cx.doc.warn(format!(
                        "an embedded object on {} is damaged; only its picture is shown",
                        self.location
                    ));
                    return None;
                };
                // Nothing was saved; the shape's picture is all there is.
                if object::is_empty_storage(&data) {
                    return None;
                }
                let part = self
                    .cx
                    .object(data, origin, prog_id.as_deref(), self.location);
                Some(Block::Embed(EmbedRef { part }))
            }
            ExObject::Media { path } => {
                let origin = format!("link:{path}");
                if let Some(part) = self.cx.doc.part_by_origin(&origin) {
                    let part = part.id.clone();
                    return Some(Block::Embed(EmbedRef { part }));
                }
                let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
                let mut part = Part::new(PartKind::Media, Vec::new())
                    .with_link(path.clone())
                    .with_origin(origin)
                    .with_location(self.location)
                    .with_name(name);
                if let Some(mime) = name
                    .rsplit_once('.')
                    .and_then(|(_, ext)| doctopus_core::sniff::mime_for_extension(ext))
                {
                    part = part.with_content_type(mime);
                }
                let part = self.cx.doc.add_part(part);
                Some(Block::Embed(EmbedRef { part }))
            }
        }
    }

    fn picture(&mut self, index: u32, shape: &Shape<'_>) -> Option<Block> {
        let origin = format!("blip:{index}");
        let part = match self.cx.doc.part_by_origin(&origin) {
            Some(part) => part.id.clone(),
            None => {
                let max = self.cx.options.max_part_size;
                let blip = self
                    .pres
                    .store
                    .get(index, self.pres.pictures.as_deref(), max)?;
                self.cx.picture(blip, origin, self.location)
            }
        };
        Some(Block::Image(ImageRef {
            part,
            alt: shape.props.description.clone(),
        }))
    }

    /// A table: a group whose cells are text shapes laid out on a grid.
    fn table(&mut self, group: &Shape<'_>) -> Option<Table> {
        struct Cell {
            rect: [i32; 4],
            blocks: Vec<Block>,
        }
        let mut cells = Vec::new();
        for child in &group.children {
            if child.is_deleted() || child.shape_type == SHAPE_LINE || !child.children.is_empty() {
                continue;
            }
            let Some(rect) = rect(child).filter(|[l, t, r, b]| r > l && b > t) else {
                continue;
            };
            let blocks = self
                .shape_text(child)
                .map(|body| body.blocks([false; LEVELS], &self.pres.hyperlinks))
                .unwrap_or_default();
            cells.push(Cell { rect, blocks });
        }
        if cells.is_empty() {
            return None;
        }
        let cols = edges(cells.iter().map(|c| c.rect[0]));
        let rows = edges(cells.iter().map(|c| c.rect[1]));
        let mut grid = vec![vec![Slot::Empty; cols.len()]; rows.len()];
        let mut placed = Vec::new();
        for (i, cell) in cells.iter().enumerate() {
            let [left, top, right, bottom] = cell.rect;
            let (Some(row), Some(col)) = (edge_index(&rows, top), edge_index(&cols, left)) else {
                continue;
            };
            if grid[row][col] != Slot::Empty {
                continue;
            }
            let row_span = span(&rows, top, bottom).min(rows.len() - row);
            let col_span = span(&cols, left, right).min(cols.len() - col);
            for (r, line) in grid.iter_mut().enumerate().skip(row).take(row_span) {
                for (c, slot) in line.iter_mut().enumerate().skip(col).take(col_span) {
                    if *slot == Slot::Empty {
                        *slot = if (r, c) == (row, col) {
                            Slot::Start(i)
                        } else {
                            Slot::Covered(i)
                        };
                    }
                }
            }
            placed.push((i, col, row_span, col_span));
        }
        let span_of = |i: usize| {
            placed
                .iter()
                .find(|p| p.0 == i)
                .map_or((0, 1, 1), |p| (p.1, p.2, p.3))
        };
        let mut table = Table::default();
        for line in &grid {
            let mut row = TableRow::default();
            let mut c = 0;
            while c < line.len() {
                match line[c] {
                    Slot::Start(i) => {
                        let (_, row_span, col_span) = span_of(i);
                        let mut cell = TableCell::new(std::mem::take(&mut cells[i].blocks));
                        cell.row_span = row_span as u32;
                        cell.col_span = col_span as u32;
                        row.cells.push(cell);
                        c += col_span;
                    }
                    Slot::Covered(i) => {
                        let (start, _, col_span) = span_of(i);
                        let col_span = if start == c { col_span } else { 1 };
                        let mut cell = TableCell::new(Vec::new());
                        cell.col_span = col_span as u32;
                        cell.merged = true;
                        row.cells.push(cell);
                        c += col_span;
                    }
                    Slot::Empty => {
                        row.cells.push(TableCell::new(Vec::new()));
                        c += 1;
                    }
                }
            }
            table.rows.push(row);
        }
        Some(table)
    }
}

/// What occupies one position of a table grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Empty,
    /// The top-left corner of a cell.
    Start(usize),
    /// Covered by a cell that spans rows or columns.
    Covered(usize),
}

/// The bounds of a shape as `[left, top, right, bottom]`: from its child
/// anchor inside a group, else from its slide anchor.
fn rect(shape: &Shape<'_>) -> Option<[i32; 4]> {
    if let Some(anchor) = shape.anchor {
        let mut r = Cursor::new(anchor);
        return Some([r.i32()?, r.i32()?, r.i32()?, r.i32()?]);
    }
    let anchor = shape.client_anchor?;
    let mut r = Cursor::new(anchor);
    if anchor.len() >= 16 {
        let (top, left, right, bottom) = (r.i32()?, r.i32()?, r.i32()?, r.i32()?);
        Some([left, top, right, bottom])
    } else {
        let (top, left, right, bottom) = (r.i16()?, r.i16()?, r.i16()?, r.i16()?);
        Some([left, top, right, bottom].map(i32::from))
    }
}

/// Distinct coordinates, merging those closer than the tolerance.
fn edges(values: impl Iterator<Item = i32>) -> Vec<i32> {
    let mut values: Vec<i32> = values.collect();
    values.sort_unstable();
    let mut out: Vec<i32> = Vec::new();
    for v in values {
        if out.last().is_none_or(|&last| v - last > EDGE_TOLERANCE) {
            out.push(v);
        }
    }
    out
}

fn edge_index(edges: &[i32], v: i32) -> Option<usize> {
    edges.iter().position(|&e| (e - v).abs() <= EDGE_TOLERANCE)
}

/// How many grid lines a cell from `from` to `to` covers.
fn span(edges: &[i32], from: i32, to: i32) -> usize {
    edges
        .iter()
        .filter(|&&e| e >= from - EDGE_TOLERANCE && e < to - EDGE_TOLERANCE)
        .count()
        .max(1)
}

/// The compound file of an embedded object (`ExOleObjStg`), inflated when
/// compressed.
fn ole_storage(record: &Record<'_>, max_size: u64) -> Option<Vec<u8>> {
    if record.instance == 0 {
        return Some(record.data.to_vec());
    }
    let mut r = Cursor::new(record.data);
    let size = u64::from(r.u32()?);
    officeart::inflate(r.take(r.remaining())?, size.min(max_size))
}

/// The review comments of a slide, kept in its `___PPT10` tag.
fn slide_comments(slide: &Record<'_>, counter: &mut u32) -> Vec<Block> {
    let mut out = Vec::new();
    let tags = slide
        .children()
        .filter(|r| r.kind == rt::PROG_TAGS)
        .flat_map(|tags| tags.children())
        .filter(|tag| {
            tag.kind == rt::PROG_BINARY_TAG && cstring(tag, 0).as_deref() == Some("___PPT10")
        });
    for tag in tags {
        let Some(blob) = tag.child(rt::BINARY_TAG_DATA_BLOB) else {
            continue;
        };
        for comment in Records::new(blob.data).filter(|r| r.kind == rt::COMMENT10) {
            *counter += 1;
            let blocks = cstring(&comment, 1)
                .unwrap_or_default()
                .split(['\r', '\n'])
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(Block::text)
                .collect();
            let date = comment
                .child(rt::COMMENT10_ATOM)
                .and_then(|atom| system_time(atom.data.get(4..)?));
            out.push(Block::Note(Note {
                kind: NoteKind::Comment,
                label: format!("c{counter}"),
                author: cstring(&comment, 0),
                date,
                anchor: None,
                blocks,
            }));
        }
    }
    out
}

/// A SYSTEMTIME as `YYYY-MM-DD HH:MM`.
fn system_time(data: &[u8]) -> Option<String> {
    let mut r = Cursor::new(data);
    let year = r.u16()?;
    let month = r.u16()?;
    r.skip(2)?;
    let day = r.u16()?;
    let hour = r.u16()?;
    let minute = r.u16()?;
    let valid = (1601..=9999).contains(&year)
        && (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour < 24
        && minute < 60;
    valid.then(|| format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}"))
}

// ----- text --------------------------------------------------------------------

/// One body of text with its formatting.
#[derive(Debug, Clone, Default)]
struct TextBody {
    text_type: u32,
    /// UTF-16 code units; paragraphs end with `\r`, `\u{b}` breaks lines.
    units: Vec<u16>,
    paragraphs: Vec<ParagraphRun>,
    runs: Vec<CharRun>,
    links: Vec<TextLink>,
}

/// Formatting of consecutive paragraphs (`TextPFRun`).
#[derive(Debug, Clone, Copy)]
struct ParagraphRun {
    /// Characters covered, paragraph marks included.
    count: usize,
    indent: u16,
    /// Bulleted or not, when the run says so.
    bullet: Option<bool>,
}

/// Formatting of consecutive characters (`TextCFRun`).
#[derive(Debug, Clone, Copy)]
struct CharRun {
    count: usize,
    style: Style,
}

/// A hyperlink on a range of characters.
#[derive(Debug, Clone, Copy)]
struct TextLink {
    start: usize,
    end: usize,
    hyperlink: u32,
}

/// Split text records into bodies: each starts at a `TextHeaderAtom` and
/// has one text atom plus its style and link records.
fn text_bodies(records: Vec<Record<'_>>) -> Vec<TextBody> {
    let is_text = |r: &Record<'_>| matches!(r.kind, rt::TEXT_CHARS_ATOM | rt::TEXT_BYTES_ATOM);
    let mut groups: Vec<(u32, Vec<Record<'_>>)> = Vec::new();
    for record in records {
        if record.kind == rt::TEXT_HEADER_ATOM {
            let text_type = u32_at(record.data, 0).unwrap_or(text_type::OTHER);
            groups.push((text_type, Vec::new()));
            continue;
        }
        // Text without a header of its own starts another body.
        if is_text(&record) && groups.last().is_none_or(|(_, g)| g.iter().any(is_text)) {
            groups.push((text_type::OTHER, Vec::new()));
        }
        if let Some((_, group)) = groups.last_mut() {
            group.push(record);
        }
    }
    groups
        .into_iter()
        .map(|(text_type, records)| TextBody::read(text_type, &records))
        .collect()
}

/// Hyperlinks on character ranges: each `TextInteractiveInfoAtom` follows
/// the `InteractiveInfo` that says what clicking the range does.
fn text_links(records: &[Record<'_>]) -> Vec<TextLink> {
    let mut out = Vec::new();
    let mut pending = None;
    for record in records {
        match record.kind {
            rt::INTERACTIVE_INFO if record.instance == 0 => {
                pending = record
                    .child(rt::INTERACTIVE_INFO_ATOM)
                    .and_then(|a| hyperlink_ref(a.data));
            }
            rt::TEXT_INTERACTIVE_INFO_ATOM if record.instance == 0 => {
                if let (Some(hyperlink), Some(start), Some(end)) = (
                    pending.take(),
                    u32_at(record.data, 0),
                    u32_at(record.data, 4),
                ) {
                    out.push(TextLink {
                        start: start as usize,
                        end: end as usize,
                        hyperlink,
                    });
                }
            }
            _ => {}
        }
    }
    out
}

impl TextBody {
    fn read(text_type: u32, records: &[Record<'_>]) -> TextBody {
        let mut body = TextBody {
            text_type,
            ..TextBody::default()
        };
        for record in records {
            match record.kind {
                rt::TEXT_CHARS_ATOM => {
                    body.units = record
                        .data
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_le_bytes(*c))
                        .collect();
                }
                // The low bytes of UTF-16 code units whose high bytes are 0.
                rt::TEXT_BYTES_ATOM => {
                    body.units = record.data.iter().map(|&b| u16::from(b)).collect()
                }
                rt::STYLE_TEXT_PROP_ATOM => {
                    (body.paragraphs, body.runs) = style_runs(record.data, body.units.len());
                }
                _ => {}
            }
        }
        body.links = text_links(records);
        body
    }

    /// The text on one line, as a title.
    fn plain(&self) -> String {
        String::from_utf16_lossy(&self.units)
            .split(|c: char| c.is_whitespace() || c.is_control())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Paragraphs and list items. `defaults` says whether paragraphs that
    /// do not say so themselves are bulleted, by indent level.
    fn blocks(&self, defaults: [bool; LEVELS], hyperlinks: &HashMap<u32, String>) -> Vec<Block> {
        let mut out = Vec::new();
        let mut start = 0;
        loop {
            let end = self.units[start..]
                .iter()
                .position(|&u| u == 0x0D)
                .map_or(self.units.len(), |p| start + p);
            let content = self.inlines(start, end, hyperlinks);
            let paragraph = self.paragraph_at(start);
            let level = paragraph.map_or(0, |p| usize::from(p.indent).min(LEVELS - 1));
            let bullet = paragraph.and_then(|p| p.bullet).unwrap_or(defaults[level]);
            let block = if bullet {
                Block::ListItem(ListItem {
                    level: level as u8,
                    label: None,
                    content,
                })
            } else {
                Block::paragraph(content)
            };
            if !block.is_blank() {
                out.push(block);
            }
            if end >= self.units.len() {
                return out;
            }
            start = end + 1;
        }
    }

    /// The runs of `start..end`, split where formatting or links change.
    fn inlines(&self, start: usize, end: usize, hyperlinks: &HashMap<u32, String>) -> Vec<Inline> {
        let mut cuts = vec![start, end];
        let mut pos = 0usize;
        for run in &self.runs {
            pos = pos.saturating_add(run.count);
            cuts.push(pos);
        }
        cuts.extend(self.links.iter().flat_map(|l| [l.start, l.end]));
        cuts.retain(|&c| c >= start && c <= end);
        cuts.sort_unstable();
        cuts.dedup();
        let mut content: Vec<Inline> = Vec::new();
        for pair in cuts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let text = String::from_utf16_lossy(&self.units[a..b]);
            let style = self.style_at(a);
            let url = self
                .links
                .iter()
                .find(|l| l.start <= a && a < l.end)
                .and_then(|l| hyperlinks.get(&l.hyperlink));
            let Some(url) = url else {
                push_run(&mut content, &text, style);
                continue;
            };
            match content.last_mut() {
                Some(Inline::Link(link)) if link.url == *url => {
                    push_run(&mut link.content, &text, style)
                }
                _ => {
                    let mut inner = Vec::new();
                    push_run(&mut inner, &text, style);
                    if !inner.is_empty() {
                        content.push(Inline::Link(Link {
                            url: url.clone(),
                            content: inner,
                        }));
                    }
                }
            }
        }
        content
    }

    fn paragraph_at(&self, pos: usize) -> Option<&ParagraphRun> {
        let mut end = 0usize;
        for run in &self.paragraphs {
            end = end.saturating_add(run.count);
            if pos < end {
                return Some(run);
            }
        }
        self.paragraphs.last()
    }

    fn style_at(&self, pos: usize) -> Style {
        let mut end = 0usize;
        for run in &self.runs {
            end = end.saturating_add(run.count);
            if pos < end {
                return run.style;
            }
        }
        Style::default()
    }
}

/// Append text, turning vertical tabs and newlines into line breaks and
/// dropping other control characters.
fn push_run(content: &mut Vec<Inline>, text: &str, style: Style) {
    for (i, line) in text.split(['\u{b}', '\n']).enumerate() {
        if i > 0 {
            content.push(Inline::LineBreak);
        }
        let clean: String = line
            .chars()
            .filter(|&c| c == '\t' || !c.is_control())
            .collect();
        push_text(content, &clean, style);
    }
}

/// The paragraph and character runs of a `StyleTextPropAtom`. Both cover
/// the text plus one character; reading stops at the first damaged run.
fn style_runs(data: &[u8], len: usize) -> (Vec<ParagraphRun>, Vec<CharRun>) {
    let total = len + 1;
    let mut r = Cursor::new(data);
    let mut paragraphs = Vec::new();
    let mut covered = 0;
    while covered < total {
        let Some(run) = paragraph_run(&mut r) else {
            return (paragraphs, Vec::new());
        };
        covered += run.count.max(1);
        paragraphs.push(run);
    }
    let mut runs = Vec::new();
    covered = 0;
    while covered < total {
        let Some(run) = char_run(&mut r) else {
            break;
        };
        covered += run.count.max(1);
        runs.push(run);
    }
    (paragraphs, runs)
}

fn paragraph_run(r: &mut Cursor<'_>) -> Option<ParagraphRun> {
    let count = r.u32()? as usize;
    let indent = r.u16()?;
    let bullet = paragraph_props(r)?;
    Some(ParagraphRun {
        count,
        indent,
        bullet,
    })
}

fn char_run(r: &mut Cursor<'_>) -> Option<CharRun> {
    let count = r.u32()? as usize;
    let style = char_props(r)?;
    Some(CharRun { count, style })
}

/// Read a `TextPFException`, returning whether it turns bullets on or off.
fn paragraph_props(r: &mut Cursor<'_>) -> Option<Option<bool>> {
    let masks = r.u32()?;
    let mut bullet = None;
    if masks & 0xF != 0 {
        let flags = r.u16()?;
        if masks & 0x1 != 0 {
            bullet = Some(flags & 0x1 != 0);
        }
    }
    // bulletChar, bulletFontRef, bulletSize, bulletColor, textAlignment,
    // lineSpacing, spaceBefore, spaceAfter, leftMargin, indent,
    // defaultTabSize: in this order, each present when its bit is set.
    const FIELDS: [(u32, usize); 11] = [
        (0x80, 2),
        (0x10, 2),
        (0x40, 2),
        (0x20, 4),
        (0x800, 2),
        (0x1000, 2),
        (0x2000, 2),
        (0x4000, 2),
        (0x100, 2),
        (0x400, 2),
        (0x8000, 2),
    ];
    for (bit, size) in FIELDS {
        if masks & bit != 0 {
            r.skip(size)?;
        }
    }
    if masks & 0x10_0000 != 0 {
        let stops = usize::from(r.u16()?);
        r.skip(stops * 4)?;
    }
    // fontAlign, wrap flags, textDirection.
    for bits in [0x1_0000, 0xE_0000, 0x20_0000] {
        if masks & bits != 0 {
            r.skip(2)?;
        }
    }
    Some(bullet)
}

/// Read a `TextCFException`, returning the formatting worth keeping.
fn char_props(r: &mut Cursor<'_>) -> Option<Style> {
    let masks = r.u32()?;
    let mut style = Style::default();
    if masks & 0xFFFF != 0 {
        let flags = u32::from(r.u16()?) & masks;
        style.bold = flags & 0x1 != 0;
        style.italic = flags & 0x2 != 0;
    }
    // fontRef, oldEAFontRef, ansiFontRef, symbolFontRef, fontSize, color.
    const FIELDS: [(u32, usize); 6] = [
        (0x1_0000, 2),
        (0x20_0000, 2),
        (0x40_0000, 2),
        (0x80_0000, 2),
        (0x2_0000, 2),
        (0x4_0000, 4),
    ];
    for (bit, size) in FIELDS {
        if masks & bit != 0 {
            r.skip(size)?;
        }
    }
    if masks & 0x8_0000 != 0 {
        // Baseline offset in percent of the font size.
        let position = r.i16()?;
        style.superscript = position > 0;
        style.subscript = position < 0;
    }
    Some(style)
}

/// Whether paragraphs are bulleted by default, by text type and indent
/// level, as the master's text styles say.
#[derive(Debug, Clone, Copy, Default)]
struct MasterBullets([[Option<bool>; LEVELS]; 9]);

impl MasterBullets {
    fn read(master: &Record<'_>) -> MasterBullets {
        let mut out = MasterBullets::default();
        for style in master
            .children()
            .filter(|r| r.kind == rt::TEXT_MASTER_STYLE_ATOM)
        {
            let Some(levels) = out.0.get_mut(usize::from(style.instance)) else {
                continue;
            };
            let mut r = Cursor::new(style.data);
            let count = r.u16().unwrap_or(0);
            for i in 0..count.min(LEVELS as u16) {
                // Styles derived from another one name the level they set.
                let level = if style.instance >= 5 {
                    r.u16()
                } else {
                    Some(i)
                };
                let (Some(level), Some(bullet)) = (level, paragraph_props(&mut r)) else {
                    break;
                };
                if char_props(&mut r).is_none() {
                    break;
                }
                if let Some(slot) = levels.get_mut(usize::from(level)) {
                    *slot = bullet;
                }
            }
        }
        out
    }

    fn defaults(&self, text_type: u32) -> [bool; LEVELS] {
        let own = self.0.get(text_type as usize);
        let base = match text_type {
            text_type::CENTER_BODY | text_type::HALF_BODY | text_type::QUARTER_BODY => {
                Some(&self.0[text_type::BODY as usize])
            }
            text_type::CENTER_TITLE => Some(&self.0[text_type::TITLE as usize]),
            _ => None,
        };
        let fallback = matches!(
            text_type,
            text_type::BODY | text_type::HALF_BODY | text_type::QUARTER_BODY
        );
        std::array::from_fn(|level| {
            own.and_then(|l| l[level])
                .or_else(|| base.and_then(|b| b[level]))
                .unwrap_or(fallback)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(ver: u16, instance: u16, kind: u16, body: &[u8]) -> Vec<u8> {
        let mut out = (ver | instance << 4).to_le_bytes().to_vec();
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    fn le(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn the_edit_chain_resolves_the_newest_objects() {
        // Save 1: objects 1 and 2. Save 2: a new version of object 2.
        let mut stream = rec(0, 0, rt::DOCUMENT, &[]);
        let old = stream.len();
        stream.extend(rec(0, 0, rt::SLIDE, &[1]));
        let new = stream.len();
        stream.extend(rec(0, 0, rt::SLIDE, &[2]));
        let dir1 = stream.len();
        stream.extend(rec(
            0,
            0,
            rt::PERSIST_DIRECTORY_ATOM,
            &le(&[1 | 2 << 20, 0, old as u32]),
        ));
        let edit1 = stream.len();
        let atom = le(&[0, 0, 0, dir1 as u32, 1, 3, 0]);
        stream.extend(rec(0, 0, rt::USER_EDIT_ATOM, &atom));
        let dir2 = stream.len();
        stream.extend(rec(
            0,
            0,
            rt::PERSIST_DIRECTORY_ATOM,
            &le(&[2 | 1 << 20, new as u32]),
        ));
        let edit2 = stream.len();
        let atom = le(&[0, 0, edit1 as u32, dir2 as u32, 1, 3, 0]);
        stream.extend(rec(0, 0, rt::USER_EDIT_ATOM, &atom));

        let mut user = le(&[20, 0xE391_C05F, edit2 as u32]);
        user.extend([0; 8]);
        let current = rec(0, 0, rt::CURRENT_USER_ATOM, &user);
        let persist = Persist::read(&stream, Some(&current));
        assert!(!persist.damaged && !persist.encrypted);
        assert_eq!(
            persist.record(&stream, 1).map(|r| r.kind),
            Some(rt::DOCUMENT)
        );
        assert_eq!(persist.record(&stream, 2).map(|r| r.data), Some(&[2u8][..]));

        // Without the Current User stream the last edit is found by a scan.
        let persist = Persist::read(&stream, None);
        assert_eq!(persist.record(&stream, 2).map(|r| r.data), Some(&[2u8][..]));

        let mut user = le(&[20, ENCRYPTED_TOKEN, edit2 as u32]);
        user.extend([0; 8]);
        let current = rec(0, 0, rt::CURRENT_USER_ATOM, &user);
        assert!(Persist::read(&stream, Some(&current)).encrypted);
    }

    fn text_records(text: &str, text_type: u32, style: &[u8]) -> Vec<u8> {
        let mut out = rec(0, 0, rt::TEXT_HEADER_ATOM, &text_type.to_le_bytes());
        let units: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        out.extend(rec(0, 0, rt::TEXT_CHARS_ATOM, &units));
        if !style.is_empty() {
            out.extend(rec(0, 0, rt::STYLE_TEXT_PROP_ATOM, style));
        }
        out
    }

    #[test]
    fn paragraphs_bullets_and_styles() {
        // "Intro\rPoint\u{b}two": the first paragraph has its bullet turned
        // off, the second is at level 1 and inherits it; "Intro" is bold.
        let mut style = Vec::new();
        style.extend(6u32.to_le_bytes());
        style.extend(0u16.to_le_bytes());
        style.extend(0x1u32.to_le_bytes());
        style.extend(0u16.to_le_bytes());
        style.extend(10u32.to_le_bytes());
        style.extend(1u16.to_le_bytes());
        style.extend(0x800u32.to_le_bytes());
        style.extend(1u16.to_le_bytes());
        style.extend(5u32.to_le_bytes());
        style.extend(0x1u32.to_le_bytes());
        style.extend(0x1u16.to_le_bytes());
        style.extend(11u32.to_le_bytes());
        style.extend(0u32.to_le_bytes());
        let data = text_records("Intro\rPoint\u{b}two", text_type::BODY, &style);
        let bodies = text_bodies(Records::new(&data).collect());
        assert_eq!(bodies.len(), 1);
        let blocks = bodies[0].blocks([true; LEVELS], &HashMap::new());
        assert_eq!(blocks.len(), 2);
        let Block::Paragraph(intro) = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert!(matches!(&intro.content[0], Inline::Text(t) if t.text == "Intro" && t.style.bold));
        let Block::ListItem(item) = &blocks[1] else {
            panic!("{blocks:?}");
        };
        assert_eq!(item.level, 1);
        assert!(matches!(item.content[1], Inline::LineBreak));
    }

    #[test]
    fn character_ranges_link_to_hyperlinks() {
        let mut data = text_records("see docs here", text_type::OTHER, &[]);
        let mut atom = le(&[0, 7]);
        atom.extend([ACTION_HYPERLINK, 0, 0, 0, 0, 0, 0, 0]);
        data.extend(rec(
            0xF,
            0,
            rt::INTERACTIVE_INFO,
            &rec(0, 0, rt::INTERACTIVE_INFO_ATOM, &atom),
        ));
        data.extend(rec(0, 0, rt::TEXT_INTERACTIVE_INFO_ATOM, &le(&[4, 8])));
        let bodies = text_bodies(Records::new(&data).collect());
        let links = HashMap::from([(7, "https://example.com/".to_string())]);
        let blocks = bodies[0].blocks([false; LEVELS], &links);
        let Block::Paragraph(p) = &blocks[0] else {
            panic!("{blocks:?}");
        };
        assert!(matches!(&p.content[1], Inline::Link(l) if l.url == "https://example.com/"));
        assert_eq!(doctopus_core::plain_text(&p.content), "see docs here");
    }

    #[test]
    fn master_styles_set_bullet_defaults() {
        // Body: level 1 bulleted; title: no bullets.
        let mut body = 1u16.to_le_bytes().to_vec();
        body.extend(0x1u32.to_le_bytes());
        body.extend(0x1u16.to_le_bytes());
        body.extend(0u32.to_le_bytes());
        let mut master = rec(0, 1, rt::TEXT_MASTER_STYLE_ATOM, &body);
        let mut centered = 1u16.to_le_bytes().to_vec();
        centered.extend(0u16.to_le_bytes());
        centered.extend(0x1u32.to_le_bytes());
        centered.extend(0u16.to_le_bytes());
        centered.extend(0u32.to_le_bytes());
        master.extend(rec(0, 5, rt::TEXT_MASTER_STYLE_ATOM, &centered));
        let data = rec(0xF, 0, rt::MAIN_MASTER, &master);
        let bullets = MasterBullets::read(&Records::new(&data).next().unwrap());
        assert!(bullets.defaults(text_type::BODY)[0]);
        assert!(!bullets.defaults(text_type::CENTER_BODY)[0]);
        assert!(bullets.defaults(text_type::HALF_BODY)[0]);
        assert!(!bullets.defaults(text_type::OTHER)[0]);
        assert!(MasterBullets::default().defaults(text_type::BODY)[2]);
    }

    #[test]
    fn hyperlink_targets() {
        assert_eq!(
            hyperlink_url(Some("https://a.example/".into()), None).as_deref(),
            Some("https://a.example/")
        );
        assert_eq!(
            hyperlink_url(None, Some("258,3,Results".into())).as_deref(),
            Some("#slide-3")
        );
        assert_eq!(
            hyperlink_url(None, Some("_ftn1".into())).as_deref(),
            Some("#_ftn1")
        );
    }

    #[test]
    fn table_cells_line_up_on_a_grid() {
        assert_eq!(edges([0, 100, 3, 200].into_iter()), vec![0, 100, 200]);
        assert_eq!(span(&[0, 100, 200], 0, 200), 2);
        assert_eq!(span(&[0, 100, 200], 100, 205), 1);
        assert_eq!(edge_index(&[0, 100, 200], 103), Some(1));
    }

    #[test]
    fn comment_dates() {
        let data: Vec<u8> = [2024u16, 3, 0, 9, 14, 5, 0, 0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(system_time(&data).as_deref(), Some("2024-03-09 14:05"));
        assert_eq!(system_time(&[0; 16]), None);
    }
}
