//! Visio drawings (VSDX, Visio 2013 and later): pages of shapes and the
//! connectors between them.
//!
//! A shape dropped from a stencil stores only what differs from its master,
//! so text, shape data, hyperlinks, pictures and position are merged from the
//! master shape. Connectors become connections between the shapes they
//! join (see [`crate::diagram`]).

use std::collections::{HashMap, HashSet};

use doctopus_core::{
    Block, Diagram, Document, Input, Note, NoteKind, PartId, Result, Section, SectionKind, Shape,
};

use super::context::Context;
use super::package::Relationships;
use super::short_datetime;
use super::xml::{Element, Node};
use crate::diagram::{
    GEOMETRY, Geometry, Glue, Path, Point, ShapeInfo, diagram, format_number, strip_instance_suffix,
};

/// Shapes read per page or master; the rest are dropped with a warning.
const MAX_SHAPES: usize = 100_000;

/// Group nesting followed at most.
const MAX_DEPTH: usize = 64;

/// Elements whose text keeps its whitespace.
const MIXED: &[&str] = &["vis:Text"];

/// Read a Visio drawing.
pub(crate) fn read(input: &Input<'_>) -> Result<Document> {
    let mut cx = Context::open(input)?;
    let (main, _) = cx.main_xml()?;
    let rels = cx.rels(&main);
    let mut masters = Masters::index(&mut cx, &rels);
    let pages = pages(&mut cx, &rels);
    let mut comments = comments(&mut cx, &rels);
    let names: HashMap<&str, &str> = pages
        .iter()
        .map(|p| (p.id.as_str(), p.name.as_str()))
        .collect();
    let mut sections = Vec::new();
    let mut number = 0;
    for page in &pages {
        number += 1;
        let mut section = Section::new(SectionKind::Page)
            .with_number(number)
            .with_name(page.name.clone());
        if page.background {
            section
                .properties
                .insert("type".into(), "background page".into());
        }
        if let Some(back) = page.back_page.as_deref().and_then(|id| names.get(id)) {
            section
                .properties
                .insert("background".into(), (*back).to_string());
        }
        let mut texts = HashMap::new();
        if let Some(part) = &page.part {
            let location = format!("page \"{}\"", page.name);
            if let Some(diagram) = read_page(&mut cx, &mut masters, part, &location, &mut texts) {
                section.blocks.push(Block::Diagram(diagram));
            }
        }
        for (n, comment) in comments
            .remove(&page.id)
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            section.blocks.push(comment.into_note(n + 1, &texts));
        }
        sections.push(section);
    }
    if pages.is_empty() && !masters.index.is_empty() {
        sections.push(stencil(&mut cx, &mut masters));
    }
    cx.doc.sections = sections;
    Ok(cx.doc)
}

// ----- pages -----------------------------------------------------------------

struct PageRef {
    id: String,
    name: String,
    part: Option<String>,
    background: bool,
    back_page: Option<String>,
}

fn pages(cx: &mut Context<'_>, rels: &Relationships) -> Vec<PageRef> {
    let Some(part) = rels.first_target("pages").map(str::to_string) else {
        return Vec::new();
    };
    let Some(root) = cx.xml(&part) else {
        return Vec::new();
    };
    let page_rels = cx.rels(&part);
    root.children_named("vis:Page")
        .enumerate()
        .map(|(i, p)| PageRef {
            id: p.attr("ID").unwrap_or_default().to_string(),
            name: p
                .attr("Name")
                .or_else(|| p.attr("NameU"))
                .map_or_else(|| format!("Page-{}", i + 1), str::to_string),
            part: p
                .child("vis:Rel")
                .and_then(|r| r.attr("r:id"))
                .and_then(|id| page_rels.target(id))
                .map(str::to_string),
            background: p.attr_bool("Background").unwrap_or(false),
            back_page: p.attr("BackPage").map(str::to_string),
        })
        .collect()
}

/// Read one page into a diagram; `texts` receives each shape's text for
/// comment anchors.
fn read_page(
    cx: &mut Context<'_>,
    masters: &mut Masters,
    part: &str,
    location: &str,
    texts: &mut HashMap<String, String>,
) -> Option<Diagram> {
    let root = cx.pkg.xml_with(part, &mut cx.doc, MIXED)?;
    if !root.is("vis:PageContents") {
        cx.doc.warn(format!("`{part}` is not a drawing page"));
        return None;
    }
    let mut count = 0;
    let defs = root
        .child("vis:Shapes")
        .map(|s| ShapeDef::parse_all(s, 0, &mut count))
        .unwrap_or_default();
    if count > MAX_SHAPES {
        cx.doc.warn(format!(
            "{location} has more than {MAX_SHAPES} shapes; the rest were skipped"
        ));
    }
    let mut used = HashSet::new();
    for def in &defs {
        def.collect_masters(&mut used);
    }
    for id in used {
        masters.load(cx, &id);
    }
    let rels = cx.rels(part);
    let mut builder = PageBuilder {
        cx,
        masters: &*masters,
        rels: &rels,
        location,
        info: HashMap::new(),
    };
    let shapes: Vec<Shape> = defs.iter().map(|d| builder.shape(d, None, None)).collect();
    let info = builder.info;
    for (id, shape) in &info {
        if !shape.text.is_empty() {
            texts.insert(id.clone(), shape.text.clone());
        }
    }
    // Files from other tools often leave connectors unglued.
    diagram(shapes, &info, &glue(&root), true)
}

/// Glued connector ends: connector id → (shape at the begin, shape at the end).
fn glue(root: &Element) -> Glue {
    let mut out = Glue::new();
    for connect in root
        .child("vis:Connects")
        .into_iter()
        .flat_map(|c| c.children_named("vis:Connect"))
    {
        let (Some(from), Some(cell), Some(to)) = (
            connect.attr("FromSheet"),
            connect.attr("FromCell"),
            connect.attr("ToSheet"),
        ) else {
            continue;
        };
        let entry = out.entry(from.to_string()).or_default();
        match cell {
            "BeginX" | "BeginY" => entry.0 = Some(to.to_string()),
            "EndX" | "EndY" => entry.1 = Some(to.to_string()),
            _ => {}
        }
    }
    out
}

// ----- shapes ----------------------------------------------------------------

/// A shape as stored: only what it sets itself.
#[derive(Debug, Clone, Default)]
struct ShapeDef {
    id: String,
    name: Option<String>,
    custom_name: bool,
    guide: bool,
    master: Option<String>,
    master_shape: Option<String>,
    text: Option<Vec<TextPiece>>,
    fields: HashMap<String, Value>,
    data: Vec<Row>,
    links: Vec<Row>,
    foreign: Option<Foreign>,
    cells: HashMap<&'static str, f64>,
    /// First and last point of the first geometry path, in local coordinates.
    path: Option<Path>,
    children: Vec<ShapeDef>,
}

#[derive(Debug, Clone)]
enum TextPiece {
    Text(String),
    /// A text field: its index in the Field section and the cached result.
    Field {
        ix: String,
        shown: String,
    },
}

/// A cell value and its unit.
#[derive(Debug, Clone, Default)]
struct Value {
    v: String,
    unit: Option<String>,
}

/// A named ShapeSheet row (shape data, hyperlinks).
#[derive(Debug, Clone, Default)]
struct Row {
    name: String,
    deleted: bool,
    cells: Vec<(String, Value)>,
}

impl Row {
    fn cell(&self, name: &str) -> Option<&Value> {
        self.cells.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

/// A picture or embedded object.
#[derive(Debug, Clone)]
struct Foreign {
    object: bool,
    rel: String,
    /// The master whose relationships `rel` belongs to; `None` for the page.
    master: Option<String>,
}

impl ShapeDef {
    fn parse_all(shapes: &Element, depth: usize, count: &mut usize) -> Vec<ShapeDef> {
        let mut out = Vec::new();
        if depth > MAX_DEPTH {
            return out;
        }
        // `Del` marks a master's sub-shape removed from this instance.
        for el in shapes
            .children_named("vis:Shape")
            .filter(|el| !el.attr_bool("Del").unwrap_or(false))
        {
            *count += 1;
            if *count > MAX_SHAPES {
                break;
            }
            out.push(Self::parse(el, depth, count));
        }
        out
    }

    fn parse(el: &Element, depth: usize, count: &mut usize) -> ShapeDef {
        let mut def = ShapeDef {
            id: el.attr("ID").unwrap_or_default().to_string(),
            name: el
                .attr("Name")
                .or_else(|| el.attr("NameU"))
                .map(str::to_string),
            custom_name: el.attr_bool("IsCustomName").unwrap_or(false),
            guide: el.attr("Type") == Some("Guide"),
            master: el.attr("Master").map(str::to_string),
            master_shape: el.attr("MasterShape").map(str::to_string),
            ..ShapeDef::default()
        };
        for child in el.children() {
            match child.name() {
                "vis:Cell" => {
                    let name = child.attr("N").unwrap_or_default();
                    if let Some(&n) = GEOMETRY.iter().find(|g| **g == name)
                        && let Some(v) = child.attr_parse::<f64>("V").filter(|v| v.is_finite())
                    {
                        def.cells.insert(n, v);
                    }
                }
                "vis:Section" => match child.attr("N") {
                    Some("Property") => def.data = rows(child),
                    Some("Hyperlink") => def.links = rows(child),
                    Some("Geometry") if def.path.is_none() => def.path = path(child, &def.cells),
                    Some("Field") => {
                        for row in child.children_named("vis:Row") {
                            let ix = row.attr("IX").unwrap_or("0").to_string();
                            if let Some(value) = row
                                .children_named("vis:Cell")
                                .find(|c| c.attr("N") == Some("Value"))
                            {
                                def.fields.insert(ix, cell_value(value));
                            }
                        }
                    }
                    _ => {}
                },
                "vis:Text" => def.text = Some(text_pieces(child)),
                "vis:ForeignData" => {
                    def.foreign = child
                        .child("vis:Rel")
                        .and_then(|r| r.attr("r:id"))
                        .map(|rel| Foreign {
                            object: child.attr("ForeignType") == Some("Object"),
                            rel: rel.to_string(),
                            master: None,
                        });
                }
                "vis:Shapes" => def.children = Self::parse_all(child, depth + 1, count),
                _ => {}
            }
        }
        def
    }

    fn collect_masters(&self, out: &mut HashSet<String>) {
        if let Some(m) = &self.master {
            out.insert(m.clone());
        }
        for child in &self.children {
            child.collect_masters(out);
        }
    }
}

fn rows(section: &Element) -> Vec<Row> {
    section
        .children_named("vis:Row")
        .map(|row| Row {
            name: row
                .attr("N")
                .or_else(|| row.attr("IX"))
                .unwrap_or_default()
                .to_string(),
            deleted: row.attr_bool("Del").unwrap_or(false),
            cells: row
                .children_named("vis:Cell")
                .filter_map(|c| Some((c.attr("N")?.to_string(), cell_value(c))))
                .collect(),
        })
        .collect()
}

/// The end points of a geometry section. Relative rows are scaled by the
/// shape's size; `None` when a point is not stored on the shape itself.
fn path(section: &Element, cells: &HashMap<&'static str, f64>) -> Option<Path> {
    let mut points = section
        .children_named("vis:Row")
        .filter(|row| !row.attr_bool("Del").unwrap_or(false))
        .filter(|row| {
            row.attr("T")
                .is_some_and(|t| t.ends_with("To") || t.starts_with("Spline"))
        });
    let point = |row: &Element| -> Option<Point> {
        let cell = |n: &str| {
            row.children_named("vis:Cell")
                .find(|c| c.attr("N") == Some(n))?
                .attr_parse::<f64>("V")
        };
        let (x, y) = (cell("X")?, cell("Y")?);
        if row.attr("T")?.starts_with("Rel") {
            Some((x * cells.get("Width")?, y * cells.get("Height")?))
        } else {
            Some((x, y))
        }
    };
    let first = point(points.next()?)?;
    let last = point(points.last()?)?;
    Some(Path { first, last })
}

fn cell_value(cell: &Element) -> Value {
    // "No Formula" marks a value that was never entered.
    let unset = cell.attr("F") == Some("No Formula");
    Value {
        v: if unset {
            String::new()
        } else {
            cell.attr("V").unwrap_or_default().to_string()
        },
        unit: cell.attr("U").map(str::to_string),
    }
}

fn text_pieces(text: &Element) -> Vec<TextPiece> {
    let mut out = Vec::new();
    for node in text.nodes() {
        match node {
            Node::Text(t) => out.push(TextPiece::Text(t.clone())),
            Node::Element(e) if e.is("vis:fld") => out.push(TextPiece::Field {
                ix: e.attr("IX").unwrap_or("0").to_string(),
                shown: e.text(),
            }),
            // Character, paragraph and tab run markers.
            Node::Element(_) => {}
        }
    }
    out
}

struct PageBuilder<'c, 'a, 'm> {
    cx: &'c mut Context<'a>,
    masters: &'m Masters,
    rels: &'m Relationships,
    location: &'m str,
    info: HashMap<String, ShapeInfo>,
}

impl PageBuilder<'_, '_, '_> {
    /// Build a shape, merging in what it inherits. `context` is the master
    /// of the nearest instance this shape belongs to.
    fn shape(&mut self, def: &ShapeDef, context: Option<&str>, parent: Option<&str>) -> Shape {
        let masters = self.masters;
        let context = def.master.as_deref().or(context);
        let base = match (&def.master, &def.master_shape, context) {
            (Some(m), _, _) => masters.top_shape(m),
            (None, Some(s), Some(m)) => masters.shape(m, s),
            _ => None,
        };
        let merged = merge(def, base, context);
        // Sub-shapes of an instance carry their master's internal names,
        // which say nothing to a reader.
        let name = match &def.master {
            Some(m) => masters.name(m),
            None => def
                .name
                .as_deref()
                .filter(|_| def.custom_name && def.master_shape.is_none()),
        };
        let kind = name
            .map(|n| strip_instance_suffix(n).trim().to_string())
            .filter(|n| !n.is_empty());
        let part = merged
            .foreign
            .as_ref()
            .and_then(|f| self.foreign(f, &def.id));
        let shape_id = def.id.clone();
        let children: Vec<Shape> = def
            .children
            .iter()
            .filter(|c| !c.guide)
            .map(|c| self.shape(c, context, Some(&shape_id)))
            .collect();
        self.info.insert(
            def.id.clone(),
            ShapeInfo {
                parent: parent.map(str::to_string),
                visible: kind.is_some() || !merged.text.is_empty(),
                text: merged.text.clone(),
                geometry: Geometry {
                    cells: merged.cells,
                    path: merged.path,
                },
            },
        );
        Shape {
            id: def.id.clone(),
            kind,
            text: merged.text,
            data: merged.data,
            links: merged.links,
            part,
            children,
        }
    }

    fn foreign(&mut self, foreign: &Foreign, shape: &str) -> Option<PartId> {
        let masters = self.masters;
        let rels = match &foreign.master {
            Some(m) => &masters.master(m)?.rels,
            None => self.rels,
        };
        let location = format!("{} shape {shape}", self.location);
        if foreign.object {
            self.cx.object(rels, &foreign.rel, None, &location)
        } else {
            self.cx.image(rels, &foreign.rel, &location)
        }
    }
}

/// A shape with its inheritance applied.
struct Merged {
    text: String,
    data: Vec<(String, String)>,
    links: Vec<String>,
    foreign: Option<Foreign>,
    cells: HashMap<&'static str, f64>,
    path: Option<Path>,
}

fn merge(def: &ShapeDef, base: Option<&ShapeDef>, master: Option<&str>) -> Merged {
    let text = match (&def.text, base.and_then(|b| b.text.as_ref())) {
        (Some(own), _) => render_text(own, &def.fields, true),
        (None, Some(inherited)) => {
            let mut fields = base.map(|b| b.fields.clone()).unwrap_or_default();
            fields.extend(def.fields.iter().map(|(k, v)| (k.clone(), v.clone())));
            render_text(inherited, &fields, false)
        }
        (None, None) => String::new(),
    };
    let data = merge_rows(
        base.map(|b| b.data.as_slice()).unwrap_or_default(),
        &def.data,
    )
    .into_iter()
    .filter(|row| row.cell("Invisible").is_none_or(|v| v.v != "1"))
    .filter_map(|row| {
        let value = row
            .cell("Value")
            .map(format_value)
            .filter(|v| !v.is_empty())?;
        let label = row
            .cell("Label")
            .map(|l| l.v.trim().to_string())
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| row.name.clone());
        Some((label, value))
    })
    .collect();
    let links = merge_rows(
        base.map(|b| b.links.as_slice()).unwrap_or_default(),
        &def.links,
    )
    .into_iter()
    .filter_map(|row| {
        let address = row.cell("Address").map(|v| v.v.trim()).unwrap_or_default();
        let sub = row
            .cell("SubAddress")
            .map(|v| v.v.trim())
            .unwrap_or_default();
        match (address.is_empty(), sub.is_empty()) {
            (true, true) => None,
            (false, true) => Some(address.to_string()),
            (true, false) => Some(format!("#{sub}")),
            (false, false) => Some(format!("{address}#{sub}")),
        }
    })
    .collect();
    let foreign = def.foreign.clone().or_else(|| {
        let mut f = base?.foreign.clone()?;
        f.master = master.map(str::to_string);
        Some(f)
    });
    let mut cells = base.map(|b| b.cells.clone()).unwrap_or_default();
    cells.extend(def.cells.iter().map(|(k, v)| (*k, *v)));
    let path = def.path.or_else(|| base?.path);
    Merged {
        text,
        data,
        links,
        foreign,
        cells,
        path,
    }
}

/// Rows of a shape over those of its master: same-named rows are merged
/// cell by cell, deleted rows removed.
fn merge_rows(base: &[Row], own: &[Row]) -> Vec<Row> {
    let mut out: Vec<Row> = base.iter().filter(|r| !r.deleted).cloned().collect();
    for row in own {
        match out.iter().position(|r| r.name == row.name) {
            Some(i) if row.deleted => {
                out.remove(i);
            }
            Some(i) => {
                for (name, value) in &row.cells {
                    match out[i].cells.iter_mut().find(|(n, _)| n == name) {
                        Some(cell) => cell.1 = value.clone(),
                        None => out[i].cells.push((name.clone(), value.clone())),
                    }
                }
            }
            None if !row.deleted => out.push(row.clone()),
            None => {}
        }
    }
    out
}

/// Shape text. Fields show their cached result when the text is the
/// shape's own; inherited text shows the shape's own field values.
fn render_text(pieces: &[TextPiece], fields: &HashMap<String, Value>, own: bool) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            TextPiece::Text(t) => out.push_str(t),
            TextPiece::Field { ix, shown } => {
                let value = fields.get(ix).map(format_value).filter(|v| !v.is_empty());
                match (own, value) {
                    (true, _) if !shown.is_empty() => out.push_str(shown),
                    (_, Some(v)) => out.push_str(&v),
                    _ => out.push_str(shown),
                }
            }
        }
    }
    let out: String = out
        .replace("\r\n", "\n")
        .chars()
        .filter_map(|c| match c {
            '\r' | '\u{2028}' | '\u{2029}' => Some('\n'),
            '\u{fffc}' => None,
            c => Some(c),
        })
        .collect();
    out.trim().to_string()
}

/// A cell value as shown: lengths converted from internal inches to their
/// unit, booleans spelled out, strings as they are.
fn format_value(value: &Value) -> String {
    let v = value.v.trim();
    let unit = value.unit.as_deref().unwrap_or_default();
    let scale = match unit {
        "MM" => Some((25.4, " mm")),
        "CM" => Some((2.54, " cm")),
        "M" => Some((0.0254, " m")),
        "IN" | "IN_F" => Some((1.0, " in")),
        "FT" | "FT_I" => Some((1.0 / 12.0, " ft")),
        "PT" => Some((72.0, " pt")),
        _ => None,
    };
    match (unit, scale, v.parse::<f64>()) {
        ("BOOL", _, Ok(n)) => if n != 0.0 { "TRUE" } else { "FALSE" }.to_string(),
        (_, Some((factor, suffix)), Ok(n)) => format!("{}{suffix}", format_number(n * factor)),
        _ => v.to_string(),
    }
}

// ----- masters ---------------------------------------------------------------

struct MasterRef {
    name: Option<String>,
    part: Option<String>,
}

struct Master {
    rels: Relationships,
    shapes: HashMap<String, ShapeDef>,
    /// The master's shape when it has exactly one top-level shape.
    top: Option<String>,
}

#[derive(Default)]
struct Masters {
    index: Vec<(String, MasterRef)>,
    loaded: HashMap<String, Option<Master>>,
}

impl Masters {
    fn index(cx: &mut Context<'_>, rels: &Relationships) -> Masters {
        let Some(part) = rels.first_target("masters").map(str::to_string) else {
            return Masters::default();
        };
        let Some(root) = cx.xml(&part) else {
            return Masters::default();
        };
        let master_rels = cx.rels(&part);
        let index = root
            .children_named("vis:Master")
            .filter_map(|m| {
                let id = m.attr("ID")?.to_string();
                let name = m
                    .attr("Name")
                    .or_else(|| m.attr("NameU"))
                    .map(str::to_string);
                let part = m
                    .child("vis:Rel")
                    .and_then(|r| r.attr("r:id"))
                    .and_then(|id| master_rels.target(id))
                    .map(str::to_string);
                Some((id, MasterRef { name, part }))
            })
            .collect();
        Masters {
            index,
            loaded: HashMap::new(),
        }
    }

    fn get_ref(&self, id: &str) -> Option<&MasterRef> {
        self.index.iter().find(|(i, _)| i == id).map(|(_, m)| m)
    }

    fn name(&self, id: &str) -> Option<&str> {
        self.get_ref(id)?.name.as_deref()
    }

    fn load(&mut self, cx: &mut Context<'_>, id: &str) {
        if self.loaded.contains_key(id) {
            return;
        }
        let part = self.get_ref(id).and_then(|m| m.part.clone());
        let master = part.and_then(|part| {
            let root = cx.pkg.xml_with(&part, &mut cx.doc, MIXED)?;
            let mut count = 0;
            let top = root
                .child("vis:Shapes")
                .map(|s| ShapeDef::parse_all(s, 0, &mut count))
                .unwrap_or_default();
            let mut shapes = HashMap::new();
            let single = (top.len() == 1).then(|| top[0].id.clone());
            let mut stack = top;
            while let Some(mut def) = stack.pop() {
                // Shapes inside a master do not refer to other masters.
                def.master = None;
                stack.extend(std::mem::take(&mut def.children));
                shapes.insert(def.id.clone(), def);
            }
            Some(Master {
                rels: cx.rels(&part),
                shapes,
                top: single,
            })
        });
        self.loaded.insert(id.to_string(), master);
    }

    fn master(&self, id: &str) -> Option<&Master> {
        self.loaded.get(id)?.as_ref()
    }

    fn top_shape(&self, id: &str) -> Option<&ShapeDef> {
        let master = self.master(id)?;
        master.shapes.get(master.top.as_ref()?)
    }

    fn shape(&self, master: &str, shape: &str) -> Option<&ShapeDef> {
        self.master(master)?.shapes.get(shape)
    }
}

/// A stencil (no pages): its masters as one diagram.
fn stencil(cx: &mut Context<'_>, masters: &mut Masters) -> Section {
    let ids: Vec<String> = masters.index.iter().map(|(id, _)| id.clone()).collect();
    let mut shapes = Vec::new();
    for id in &ids {
        masters.load(cx, id);
        let base = masters.top_shape(id);
        let merged = base.map(|b| merge(b, None, None));
        shapes.push(Shape {
            id: id.clone(),
            kind: masters.name(id).map(str::to_string),
            text: merged.as_ref().map(|m| m.text.clone()).unwrap_or_default(),
            data: merged.map(|m| m.data).unwrap_or_default(),
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

// ----- comments --------------------------------------------------------------

struct Comment {
    author: Option<String>,
    date: Option<String>,
    shape: Option<String>,
    text: String,
    done: bool,
}

impl Comment {
    fn into_note(self, n: usize, texts: &HashMap<String, String>) -> Block {
        let mut blocks = vec![Block::text(self.text)];
        if self.done {
            blocks.push(Block::text("(resolved)"));
        }
        let anchor = self.shape.map(|id| match texts.get(&id) {
            Some(text) => text.clone(),
            None => format!("shape {id}"),
        });
        Block::Note(Note {
            kind: NoteKind::Comment,
            label: format!("c{n}"),
            author: self.author,
            date: self.date,
            anchor,
            blocks,
        })
    }
}

/// Review comments by page id.
fn comments(cx: &mut Context<'_>, rels: &Relationships) -> HashMap<String, Vec<Comment>> {
    let mut out: HashMap<String, Vec<Comment>> = HashMap::new();
    let Some(part) = rels.first_target("comments").map(str::to_string) else {
        return out;
    };
    let Some(root) = cx.xml(&part) else {
        return out;
    };
    let authors: HashMap<&str, &str> = root
        .child("vis:AuthorList")
        .into_iter()
        .flat_map(|a| a.children_named("vis:AuthorEntry"))
        .filter_map(|a| Some((a.attr("ID")?, a.attr("Name")?)))
        .collect();
    for entry in root
        .child("vis:CommentList")
        .into_iter()
        .flat_map(|l| l.children_named("vis:CommentEntry"))
    {
        let text = entry.text().trim().to_string();
        if text.is_empty() {
            continue;
        }
        out.entry(entry.attr("PageID").unwrap_or_default().to_string())
            .or_default()
            .push(Comment {
                author: entry
                    .attr("AuthorID")
                    .and_then(|id| authors.get(id))
                    .map(|a| (*a).to_string()),
                date: entry.attr("Date").map(short_datetime),
                shape: entry.attr("ShapeID").map(str::to_string),
                text,
                done: entry.attr_bool("Done").unwrap_or(false),
            });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(v: &str, unit: Option<&str>) -> Value {
        Value {
            v: v.into(),
            unit: unit.map(str::to_string),
        }
    }

    #[test]
    fn values_are_shown_in_their_units() {
        assert_eq!(
            format_value(&value("0.03937007874015748", Some("MM"))),
            "1 mm"
        );
        assert_eq!(format_value(&value("1", Some("BOOL"))), "TRUE");
        assert_eq!(format_value(&value("Router A", Some("STR"))), "Router A");
        assert_eq!(format_value(&value("2.5", None)), "2.5");
    }

    #[test]
    fn rows_merge_over_the_master() {
        let row = |name: &str, cells: &[(&str, &str)], deleted| Row {
            name: name.into(),
            deleted,
            cells: cells
                .iter()
                .map(|(n, v)| (n.to_string(), value(v, None)))
                .collect(),
        };
        let base = [
            row("Cost", &[("Label", "Cost"), ("Value", "0")], false),
            row("Owner", &[("Label", "Owner"), ("Value", "")], false),
        ];
        let own = [
            row("Cost", &[("Value", "12")], false),
            row("Owner", &[], true),
            row("Extra", &[("Value", "x")], false),
        ];
        let merged = merge_rows(&base, &own);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].cell("Label").unwrap().v, "Cost");
        assert_eq!(merged[0].cell("Value").unwrap().v, "12");
        assert_eq!(merged[1].name, "Extra");
    }

    #[test]
    fn inherited_text_uses_the_shapes_field_values() {
        let pieces = vec![
            TextPiece::Text("Rack ".into()),
            TextPiece::Field {
                ix: "0".into(),
                shown: "[1]".into(),
            },
        ];
        let mut fields = HashMap::new();
        fields.insert("0".to_string(), value("42 U", Some("STR")));
        assert_eq!(render_text(&pieces, &fields, false), "Rack 42 U");
        assert_eq!(render_text(&pieces, &fields, true), "Rack [1]");
    }
}
