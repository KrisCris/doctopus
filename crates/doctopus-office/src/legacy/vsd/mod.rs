//! Visio drawings in the binary format of Visio 5 to 2010 (`.vsd`, and
//! `.vss` stencils and `.vst` templates).
//!
//! The `VisioDocument` stream holds a tree of mostly compressed streams
//! ([`stream`]), read as one tree of streams and chunks ([`tree`]). Pages
//! hold shapes. A shape dropped from a stencil refers to its master, a
//! shape of a stencil page kept in the file, and stores only what differs
//! from it: text, fields, shape data, hyperlinks, picture and position are
//! merged from the master ([`shape`]). Connectors become connections
//! between the shapes they are glued to; version 5 records no glue, so
//! there the shapes their ends touch are taken.
//!
//! Page and master names live in one name table; index tables next to the
//! page and stencil lists say which name belongs to which page.

mod shape;
mod stream;
mod tree;

use std::collections::{HashMap, HashSet};

use doctopus_core::sniff::sniff;
use doctopus_core::{
    Block, Diagram, Document, Error, Input, PartId, Result, Section, SectionKind, Shape,
};

use self::shape::{
    FOREIGN_BITMAP, FOREIGN_EMF, FOREIGN_OBJECT, Foreign, ShapeDef, hyperlinks, is_shape,
    render_text, shape_data,
};
use self::stream::{Header, Streams, Version};
use self::tree::Node;
use super::context::Context;
use super::officeart::{Blip, bmp_file};
use crate::binary::{u16_at, u32_at};
use crate::diagram::{Geometry, Glue, ShapeInfo, diagram, strip_instance_suffix};
use crate::ole::text::{ansi, utf16le};

/// Group nesting followed at most.
const MAX_GROUP_DEPTH: usize = 64;

/// Stream and chunk types.
mod kind {
    pub(super) const FOREIGN_DATA: u32 = 0x0C;
    pub(super) const OLE_LIST: u32 = 0x0D;
    pub(super) const TEXT: u32 = 0x0E;
    pub(super) const PAGE: u32 = 0x15;
    pub(super) const STENCILS: u32 = 0x1D;
    pub(super) const STENCIL_PAGE: u32 = 0x1E;
    pub(super) const OLE_DATA: u32 = 0x1F;
    pub(super) const PAGES: u32 = 0x27;
    pub(super) const NAME_LIST: u32 = 0x2C;
    pub(super) const NAME: u32 = 0x2D;
    pub(super) const NAME_LIST2: u32 = 0x32;
    pub(super) const NAME2: u32 = 0x33;
    pub(super) const NAME_IDX123: u32 = 0x34;
    pub(super) const SHAPE_GROUP: u32 = 0x47;
    pub(super) const SHAPE_SHAPE: u32 = 0x48;
    pub(super) const SHAPE_GUIDE: u32 = 0x4D;
    pub(super) const SHAPE_FOREIGN: u32 = 0x4E;
    pub(super) const FIELD_LIST: u32 = 0x66;
    pub(super) const FOREIGN_TYPE: u32 = 0x98;
    pub(super) const XFORM: u32 = 0x9B;
    pub(super) const XFORM_1D: u32 = 0x9D;
    pub(super) const TEXT_FIELD: u32 = 0xA1;
    pub(super) const MISC: u32 = 0xA4;
    pub(super) const NAME_IDX: u32 = 0xC9;
    pub(super) const PROP: u32 = 0xB6;
    pub(super) const HYPERLINK: u32 = 0xC4;
}

/// Read a Visio 5-2010 drawing.
pub(crate) fn read(input: &Input<'_>) -> Result<Document> {
    let mut cx = Context::open(input)?;
    let format = cx.doc.format.name;
    let data = cx.required_stream("/VisioDocument")?;
    let version = match stream::header(&data) {
        Header::Visio(version) => version,
        Header::Unsupported(v) => {
            return Err(Error::Unsupported {
                detected: Some(format!(
                    "Visio drawing of file version {v}; save it in a newer format to read it"
                )),
            });
        }
        Header::NotVisio => {
            return Err(Error::malformed(
                format,
                "the `VisioDocument` stream does not start with the Visio header",
            ));
        }
    };
    cx.detect_macros(&["/_VBA_PROJECT_CUR"]);
    let max_size = usize::try_from(cx.options.max_part_size).unwrap_or(usize::MAX);
    let mut streams = Streams::new(&data, version, max_size);
    let damaged = || Error::malformed(format, "the stream directory of the drawing is damaged");
    let trailer = streams.trailer().ok_or_else(damaged)?;
    let (root, cut) = tree::read(&mut streams, &trailer).ok_or_else(damaged)?;
    if cut {
        cx.doc
            .warn("the drawing lists more streams than are read; the rest were skipped");
    }
    let mut reader = Reader {
        version,
        names: names(version, &root),
        masters: HashMap::new(),
        master_order: Vec::new(),
    };
    reader.read_masters(&root);
    let pages = reader.pages(&root);
    let mut sections = Vec::new();
    for (number, page) in pages.iter().enumerate() {
        let mut section = Section::new(SectionKind::Page)
            .with_number(number as u32 + 1)
            .with_name(page.name.clone());
        if page.background {
            section
                .properties
                .insert("type".into(), "background page".into());
        }
        if let Some(back) = page
            .back
            .and_then(|id| pages.iter().find(|p| p.node.id == id))
        {
            section
                .properties
                .insert("background".into(), back.name.clone());
        }
        let location = format!("page \"{}\"", page.name);
        if let Some(diagram) = reader.page(&mut cx, page, &location) {
            section.blocks.push(Block::Diagram(diagram));
        }
        sections.push(section);
    }
    if pages.is_empty() && !reader.masters.is_empty() {
        sections.push(reader.stencil(&mut cx));
    }
    cx.doc.sections = sections;
    Ok(cx.doc)
}

/// The document's name table (page, master and style names) by index.
fn names(version: Version, root: &Node) -> HashMap<u32, String> {
    let Some(list) = root.child(kind::NAME_LIST2) else {
        return HashMap::new();
    };
    list.children_of(kind::NAME2)
        .map(|node| {
            // A value (always 1) comes before the name: 32 bits wide, 16
            // in version 5.
            let skip = if version == Version::V5 { 2 } else { 4 };
            let data = node.data().get(skip..).unwrap_or_default();
            let name = match version {
                Version::V11 => utf16le(data),
                Version::V5 | Version::V6 => {
                    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
                    ansi(&data[..end])
                }
            };
            (node.id, name)
        })
        .collect()
}

struct PageRef<'n> {
    node: &'n Node,
    name: String,
    background: bool,
    /// The index of the background page.
    back: Option<u32>,
}

/// A stencil page: a master and the shapes it is made of.
struct Master<'n> {
    name: Option<String>,
    shapes: HashMap<u32, ShapeDef<'n>>,
    /// Shapes outside any group, in order.
    top: Vec<u32>,
}

struct Reader<'n> {
    version: Version,
    names: HashMap<u32, String>,
    masters: HashMap<u32, Master<'n>>,
    master_order: Vec<u32>,
}

impl<'n> Reader<'n> {
    /// Names of the streams `parent` lists, by index, from the name index
    /// next to them.
    fn element_names(&self, parent: &Node) -> HashMap<u32, String> {
        let mut out = HashMap::new();
        for index in &parent.children {
            let data = index.data();
            let pairs: Vec<(u32, u32)> = match (index.kind, self.version) {
                (kind::NAME_IDX, Version::V5) => {
                    let count = u16_at(data, 0).map_or(0, usize::from);
                    (0..count.min(data.len() / 4))
                        .filter_map(|i| {
                            let at = 2 + 4 * i;
                            Some((
                                u32::from(u16_at(data, at)?),
                                u32::from(u16_at(data, at + 2)?),
                            ))
                        })
                        .collect()
                }
                (kind::NAME_IDX, _) => {
                    let count = u32_at(data, 0).map_or(0, |c| c as usize);
                    (0..count.min(data.len() / 13))
                        .filter_map(|i| {
                            let at = 4 + 13 * i;
                            Some((u32_at(data, at)?, u32_at(data, at + 8)?))
                        })
                        .collect()
                }
                (kind::NAME_IDX123, _) => data
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .filter_map(|c| Some((u32_at(c, 0)?, u32_at(c, 4)?)))
                    .collect(),
                _ => continue,
            };
            for (name, element) in pairs {
                if let Some(name) = self.names.get(&name) {
                    out.insert(element, name.clone());
                }
            }
        }
        out
    }

    fn read_masters(&mut self, root: &'n Node) {
        let Some(stencils) = root.child(kind::STENCILS) else {
            return;
        };
        let names = self.element_names(stencils);
        for page in stencils.children_of(kind::STENCIL_PAGE) {
            let defs = self.shapes(page);
            let top = defs
                .iter()
                .filter(|d| d.parent.is_none())
                .map(|d| d.id)
                .collect();
            self.master_order.push(page.id);
            self.masters.insert(
                page.id,
                Master {
                    name: names.get(&page.id).cloned(),
                    shapes: defs.into_iter().map(|d| (d.id, d)).collect(),
                    top,
                },
            );
        }
    }

    fn pages(&self, root: &'n Node) -> Vec<PageRef<'n>> {
        let Some(pages) = root.child(kind::PAGES) else {
            return Vec::new();
        };
        let names = self.element_names(pages);
        pages
            .children_of(kind::PAGE)
            .enumerate()
            .map(|(i, node)| {
                let back_at = if self.version == Version::V5 { 0 } else { 8 };
                let back = match self.version {
                    Version::V5 => u16_at(node.data(), back_at)
                        .filter(|&b| b != u16::MAX)
                        .map(u32::from),
                    Version::V6 | Version::V11 => {
                        u32_at(node.data(), back_at).filter(|&b| b != u32::MAX)
                    }
                };
                PageRef {
                    node,
                    name: names
                        .get(&node.id)
                        .cloned()
                        .unwrap_or_else(|| format!("Page-{}", i + 1)),
                    // A page stream whose format has the low bit clear is a
                    // background page.
                    background: node.format.is_some_and(|f| f & 1 == 0),
                    back,
                }
            })
            .collect()
    }

    /// The shapes of a page or stencil page, in order.
    fn shapes(&self, page: &'n Node) -> Vec<ShapeDef<'n>> {
        let mut nodes = Vec::new();
        page.collect(&is_shape, &|_| false, &mut nodes);
        nodes
            .into_iter()
            .map(|n| ShapeDef::read(self.version, n))
            .collect()
    }

    /// The master shape `def` is an instance of.
    fn base(&self, def: &ShapeDef<'n>) -> Option<(u32, u32, &ShapeDef<'n>)> {
        let id = def.master?;
        let master = self.masters.get(&id)?;
        let shape = def
            .master_shape
            .or_else(|| (master.top.len() == 1).then(|| master.top[0]))?;
        Some((id, shape, master.shapes.get(&shape)?))
    }

    fn page(&self, cx: &mut Context<'_>, page: &PageRef<'n>, location: &str) -> Option<Diagram> {
        let defs = self.shapes(page.node);
        let mut builder = PageBuilder {
            cx,
            reader: self,
            location,
            by_id: defs.iter().enumerate().map(|(i, d)| (d.id, i)).collect(),
            children: HashMap::new(),
            info: HashMap::new(),
            glue: HashMap::new(),
            built: HashSet::new(),
            defs: &defs,
        };
        let mut top = Vec::new();
        for (i, def) in defs.iter().enumerate() {
            match def
                .parent
                .filter(|p| *p != def.id && builder.by_id.contains_key(p))
            {
                Some(parent) => builder.children.entry(parent).or_default().push(i),
                None => top.push(i),
            }
        }
        let shapes: Vec<Shape> = top
            .into_iter()
            .filter(|&i| defs[i].kind != kind::SHAPE_GUIDE)
            .filter_map(|i| builder.shape(i, None, 0))
            .collect();
        let glue = builder.resolve_glue();
        // Visio records glue; only version 5 glue is not read, so its
        // connectors are matched to the shapes their ends touch.
        diagram(shapes, &builder.info, &glue, self.version == Version::V5)
    }

    /// A stencil without pages: its masters as one diagram.
    fn stencil(&self, cx: &mut Context<'_>) -> Section {
        let mut shapes = Vec::new();
        for id in &self.master_order {
            let master = &self.masters[id];
            let top = (master.top.len() == 1)
                .then(|| master.shapes.get(&master.top[0]))
                .flatten();
            let text = top.map(|def| self.text(def, None)).unwrap_or_default();
            let part = top.and_then(|def| {
                let foreign = def.foreign.as_ref()?;
                let location = format!("master \"{}\"", master.name.as_deref().unwrap_or("?"));
                foreign_part(
                    cx,
                    foreign,
                    format!("master {id} shape {}", def.id),
                    &location,
                )
            });
            shapes.push(Shape {
                id: id.to_string(),
                kind: master.name.clone(),
                text,
                part,
                ..Shape::default()
            });
        }
        let mut section = Section::new(SectionKind::Other).with_name("Masters");
        if !shapes.is_empty() {
            section.blocks.push(Block::Diagram(Diagram {
                shapes,
                connections: Vec::new(),
            }));
        }
        section
    }

    /// A shape's text, falling back to its master's, with fields filled in.
    fn text(&self, def: &ShapeDef<'n>, base: Option<&ShapeDef<'n>>) -> String {
        let Some(pieces) = def.text.as_ref().or_else(|| base?.text.as_ref()) else {
            return String::new();
        };
        let fields = def
            .fields
            .as_ref()
            .or_else(|| base?.fields.as_ref())
            .map(Vec::as_slice)
            .unwrap_or_default();
        let names = |id: i32| def.names.get(&id).or_else(|| base?.names.get(&id)).cloned();
        render_text(pieces, fields, &names)
    }
}

struct PageBuilder<'b, 'c, 'a, 'n> {
    cx: &'c mut Context<'a>,
    reader: &'b Reader<'n>,
    location: &'b str,
    defs: &'b [ShapeDef<'n>],
    by_id: HashMap<u32, usize>,
    /// Members of each group, by the group's id.
    children: HashMap<u32, Vec<usize>>,
    info: HashMap<String, ShapeInfo>,
    /// Connector id → the shapes its ends are glued to, in the order found.
    glue: HashMap<String, Vec<String>>,
    built: HashSet<u32>,
}

impl PageBuilder<'_, '_, '_, '_> {
    fn shape(&mut self, i: usize, parent: Option<&str>, depth: usize) -> Option<Shape> {
        let reader = self.reader;
        let def = &self.defs[i];
        if depth > MAX_GROUP_DEPTH || !self.built.insert(def.id) {
            return None;
        }
        let base = reader.base(def);
        let master = base.and_then(|(m, _, _)| reader.masters.get(&m));
        // Only the instance of a master is named after it, not the shapes
        // it is made of.
        let kind = master
            .filter(|_| base.is_some_and(|(_, _, b)| b.parent.is_none()))
            .and_then(|m| m.name.as_deref())
            .map(|n| strip_instance_suffix(n).trim().to_string())
            .filter(|n| !n.is_empty());
        let text = reader.text(def, base.map(|(_, _, b)| b));
        let data = shape_data(def, base.map(|(_, _, b)| b));
        let links = hyperlinks(def, base.map(|(_, _, b)| b));
        let part = match (
            &def.foreign,
            base.and_then(|(m, s, b)| Some((m, s, b.foreign.as_ref()?))),
        ) {
            (Some(own), _) => {
                let origin = format!("{} shape {}", self.location, def.id);
                self.foreign(own, origin, def.id)
            }
            (None, Some((m, s, inherited))) => {
                let origin = format!("master {m} shape {s}");
                self.foreign(inherited, origin, def.id)
            }
            (None, None) => None,
        };
        let mut cells = base.map(|(_, _, b)| b.cells.clone()).unwrap_or_default();
        cells.extend(def.cells.iter().map(|(k, v)| (*k, *v)));
        let id = def.id.to_string();
        let members = self.children.get(&def.id).cloned().unwrap_or_default();
        let children = members
            .into_iter()
            .filter(|&c| self.defs[c].kind != kind::SHAPE_GUIDE)
            .filter_map(|c| self.shape(c, Some(&id), depth + 1))
            .collect();
        let glued: Vec<String> = def
            .glue
            .iter()
            .filter(|g| self.by_id.contains_key(g))
            .map(u32::to_string)
            .collect();
        if !glued.is_empty() {
            self.glue.insert(id.clone(), glued);
        }
        self.info.insert(
            id.clone(),
            ShapeInfo {
                parent: parent.map(str::to_string),
                visible: kind.is_some() || !text.is_empty(),
                text: text.clone(),
                geometry: Geometry { cells, path: None },
            },
        );
        Some(Shape {
            id,
            kind,
            text,
            data,
            links,
            part,
            children,
        })
    }

    fn foreign(&mut self, foreign: &Foreign<'_>, origin: String, shape: u32) -> Option<PartId> {
        let location = format!("{} shape {shape}", self.location);
        foreign_part(self.cx, foreign, origin, &location)
    }

    /// Glue as begin and end. A connector glued at one end only records
    /// which shape, not which end: the end that touches the shape is meant.
    fn resolve_glue(&self) -> Glue {
        let mut out = Glue::new();
        for (connector, ids) in &self.glue {
            let ends = self.info.get(connector).and_then(|s| s.geometry.ends());
            let entry = match ids.as_slice() {
                [begin, end, ..] => (Some(begin.clone()), Some(end.clone())),
                [only] => {
                    let at_end = ends
                        .zip(self.info.get(only))
                        .is_some_and(|((b, e), t)| t.geometry.touches(e) && !t.geometry.touches(b));
                    if at_end {
                        (None, Some(only.clone()))
                    } else {
                        (Some(only.clone()), None)
                    }
                }
                [] => continue,
            };
            out.insert(connector.clone(), entry);
        }
        out
    }
}

/// The part for a shape's picture or embedded object.
fn foreign_part(
    cx: &mut Context<'_>,
    foreign: &Foreign<'_>,
    origin: String,
    location: &str,
) -> Option<PartId> {
    if foreign.kind == FOREIGN_OBJECT
        && let Some(list) = foreign.object
    {
        let data: Vec<u8> = list
            .children_of(kind::OLE_DATA)
            .flat_map(|n| n.data().iter().copied())
            .collect();
        if !data.is_empty() {
            return Some(cx.object(data, origin, None, location));
        }
    }
    let raw = foreign.data?.data();
    if raw.is_empty() {
        return None;
    }
    // Bitmaps are stored without their file header.
    let data = if foreign.kind == FOREIGN_BITMAP
        && matches!(foreign.format, 0 | 255)
        && !raw.starts_with(b"BM")
    {
        bmp_file(raw)?
    } else {
        raw.to_vec()
    };
    let content_type = sniff(&data)
        .map(|s| s.mime)
        .filter(|m| m.starts_with("image/"))
        .unwrap_or(match (foreign.kind, foreign.format) {
            (FOREIGN_BITMAP, 1) => "image/jpeg",
            (FOREIGN_BITMAP, 2) => "image/gif",
            (FOREIGN_BITMAP, 3) => "image/tiff",
            (FOREIGN_BITMAP, 4) => "image/png",
            (FOREIGN_BITMAP, _) => "image/bmp",
            (FOREIGN_EMF, _) => "image/emf",
            _ => "image/wmf",
        });
    let blip = Blip {
        data,
        content_type,
        name: None,
    };
    Some(cx.picture(blip, origin, location))
}
