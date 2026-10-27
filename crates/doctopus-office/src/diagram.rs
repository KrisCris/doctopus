//! What the Visio readers share: where shapes are, and which shapes the
//! connectors of a page join.
//!
//! Connectors glued to shapes connect those shapes. For connectors written
//! without glue information (common in exports from other tools), each end
//! is matched to the shape it touches, and the connection is marked as
//! inferred.

use std::collections::{HashMap, HashSet};

use doctopus_core::{Connection, Diagram, Shape};

/// How far (in inches) a connector end may lie outside a shape it touches.
const TOUCH_TOLERANCE: f64 = 0.06;

/// Parent links followed at most when naming a shape.
const MAX_DEPTH: usize = 64;

/// The ShapeSheet cells that place a shape.
pub(crate) const GEOMETRY: &[&str] = &[
    "PinX", "PinY", "Width", "Height", "LocPinX", "LocPinY", "Angle", "BeginX", "BeginY", "EndX",
    "EndY",
];

pub(crate) type Point = (f64, f64);

/// The first and last point of a shape's drawn path, in local coordinates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Path {
    pub(crate) first: Point,
    pub(crate) last: Point,
}

/// What the connection pass needs to know about a built shape.
pub(crate) struct ShapeInfo {
    pub(crate) parent: Option<String>,
    /// Whether a reader would recognize the shape (it has text or a type).
    pub(crate) visible: bool,
    pub(crate) text: String,
    pub(crate) geometry: Geometry,
}

/// Connector ends glued to shapes: connector id → (shape at the begin,
/// shape at the end).
pub(crate) type Glue = HashMap<String, (Option<String>, Option<String>)>;

/// A page's diagram: its shapes, with the connectors among them turned
/// into connections. A connector stays listed as a shape only when it
/// carries more than text. `None` for an empty page.
///
/// Glued connectors count wherever they are; with `infer`, so do the free
/// ends of top-level connectors, matched to the shapes they touch.
pub(crate) fn diagram(
    mut shapes: Vec<Shape>,
    info: &HashMap<String, ShapeInfo>,
    glue: &Glue,
    infer: bool,
) -> Option<Diagram> {
    let top: HashSet<String> = shapes.iter().map(|s| s.id.clone()).collect();
    let mut order = Vec::new();
    walk(&shapes, &mut order);
    let listed: HashSet<&str> = order.iter().map(String::as_str).collect();
    let mut connectors = HashSet::new();
    let mut connections = Vec::new();
    for id in &order {
        let Some(shape) = info.get(id) else {
            continue;
        };
        let glued = glue.get(id);
        if glued.is_none() && !(infer && top.contains(id)) {
            continue;
        }
        let Some((begin, end)) = shape.geometry.ends() else {
            continue;
        };
        let place = |p| on_page(info, shape.parent.as_deref(), p);
        let mut inferred = false;
        // Glue to a shape the page does not list (deleted, or a guide)
        // counts as none.
        let mut end_of = |glued: Option<&String>, point: Point, other: Point| match glued {
            Some(target) if listed.contains(target.as_str()) => Some(target.clone()),
            _ if infer => {
                inferred = true;
                touching(info, &order, id, place(point)?, place(other)?)
            }
            _ => None,
        };
        let from = end_of(glued.and_then(|g| g.0.as_ref()), begin, end);
        let to = end_of(glued.and_then(|g| g.1.as_ref()), end, begin);
        let (Some(from), Some(to)) = (from, to) else {
            continue;
        };
        connectors.insert(id.clone());
        let label = find(&shapes, id).map_or_else(|| shape.text.clone(), Shape::shown_text);
        connections.push(Connection {
            from,
            to,
            label: Some(label).filter(|t| !t.is_empty()),
            connector: Some(id.clone()),
            inferred,
        });
    }
    // A connector glued to another one branches off it: it comes from
    // where that one comes from, or leads where that one leads.
    let ends: HashMap<String, (String, String)> = connections
        .iter()
        .filter_map(|c| Some((c.connector.clone()?, (c.from.clone(), c.to.clone()))))
        .collect();
    for c in &mut connections {
        c.from = visible_id(info, branch(&ends, &c.from, true));
        c.to = visible_id(info, branch(&ends, &c.to, false));
    }
    drop_connectors(&mut shapes, &connectors);
    let mut kept = Vec::new();
    walk(&shapes, &mut kept);
    let kept: HashSet<&str> = kept.iter().map(String::as_str).collect();
    for c in &mut connections {
        c.connector = c.connector.take().filter(|id| kept.contains(id.as_str()));
    }
    (!shapes.is_empty() || !connections.is_empty()).then_some(Diagram {
        shapes,
        connections,
    })
}

/// The shape a connection end reaches through the connectors it is glued
/// to (`ends`: connector → its two ends), towards their begins or ends.
fn branch<'a>(
    ends: &'a HashMap<String, (String, String)>,
    mut id: &'a str,
    begin: bool,
) -> &'a str {
    for _ in 0..MAX_DEPTH {
        match ends.get(id) {
            Some((from, to)) => id = if begin { from } else { to },
            None => break,
        }
    }
    id
}

/// Shape ids in document order, groups before their members.
fn walk(shapes: &[Shape], out: &mut Vec<String>) {
    for shape in shapes {
        out.push(shape.id.clone());
        walk(&shape.children, out);
    }
}

/// The shape `id`, wherever it is nested.
fn find<'s>(shapes: &'s [Shape], id: &str) -> Option<&'s Shape> {
    shapes.iter().find_map(|s| {
        if s.id == id {
            Some(s)
        } else {
            find(&s.children, id)
        }
    })
}

/// Remove connectors that carry nothing but text: their connection says
/// it all.
fn drop_connectors(shapes: &mut Vec<Shape>, connectors: &HashSet<String>) {
    let plain = |s: &Shape| {
        s.data.is_empty()
            && s.links.is_empty()
            && s.part.is_none()
            && s.children
                .iter()
                .all(|c| c.text.is_empty() && c.data.is_empty())
    };
    shapes.retain(|s| !(connectors.contains(&s.id) && plain(s)));
    for shape in shapes {
        drop_connectors(&mut shape.children, connectors);
    }
}

/// The smallest 2-D shape that `point` touches and that does not also
/// contain the connector's other end (which would make it a container the
/// connector runs inside, not an end point). Points are on the page.
fn touching(
    info: &HashMap<String, ShapeInfo>,
    order: &[String],
    connector: &str,
    point: Point,
    other: Point,
) -> Option<String> {
    order
        .iter()
        .filter(|id| *id != connector)
        .filter_map(|id| {
            let shape = info.get(id)?;
            if shape.geometry.ends().is_some() {
                return None;
            }
            let b = page_bounds(info, shape)?;
            (b.contains(point, TOUCH_TOLERANCE) && !b.contains(other, 0.0)).then(|| (b.area(), id))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id.clone())
}

/// A point in the coordinates of shape `parent` (the page when `None`),
/// on the page: groups place their members in their own coordinates.
fn on_page(
    info: &HashMap<String, ShapeInfo>,
    parent: Option<&str>,
    mut point: Point,
) -> Option<Point> {
    let mut current = parent;
    for _ in 0..MAX_DEPTH {
        let Some(id) = current else {
            return Some(point);
        };
        let group = info.get(id)?;
        point = group.geometry.place(point)?;
        current = group.parent.as_deref();
    }
    None
}

/// A shape's bounding box on the page.
fn page_bounds(info: &HashMap<String, ShapeInfo>, shape: &ShapeInfo) -> Option<Bounds> {
    let mut b = Bounds::EMPTY;
    for corner in shape.geometry.corners()? {
        b.add(on_page(info, shape.parent.as_deref(), corner)?);
    }
    Some(b)
}

/// The shape a reader would name for `id`: itself when it has text or a
/// type, otherwise the nearest such ancestor, otherwise its top-level group.
fn visible_id(info: &HashMap<String, ShapeInfo>, id: &str) -> String {
    let mut current = id;
    let mut steps = 0;
    while let Some(shape) = info.get(current) {
        if shape.visible || steps > MAX_DEPTH {
            return current.to_string();
        }
        match &shape.parent {
            Some(parent) => current = parent,
            None => return current.to_string(),
        }
        steps += 1;
    }
    id.to_string()
}

/// `Process.12` → `Process`: Visio numbers repeated instances.
pub(crate) fn strip_instance_suffix(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((base, n))
            if !base.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) =>
        {
            base
        }
        _ => name,
    }
}

/// A number with at most six decimals and no trailing zeros.
pub(crate) fn format_number(n: f64) -> String {
    let rounded = (n * 1e6).round() / 1e6;
    let mut s = format!("{rounded:.6}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    if s == "-0" { "0".to_string() } else { s }
}

// ----- geometry --------------------------------------------------------------

/// Where a shape is, in inches in its parent's coordinates.
pub(crate) struct Geometry {
    /// Cells named in [`GEOMETRY`].
    pub(crate) cells: HashMap<&'static str, f64>,
    pub(crate) path: Option<Path>,
}

struct Bounds {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Bounds {
    const EMPTY: Bounds = Bounds {
        x0: f64::INFINITY,
        y0: f64::INFINITY,
        x1: f64::NEG_INFINITY,
        y1: f64::NEG_INFINITY,
    };

    fn add(&mut self, (x, y): Point) {
        self.x0 = self.x0.min(x);
        self.y0 = self.y0.min(y);
        self.x1 = self.x1.max(x);
        self.y1 = self.y1.max(y);
    }

    fn contains(&self, (x, y): Point, tolerance: f64) -> bool {
        x >= self.x0 - tolerance
            && x <= self.x1 + tolerance
            && y >= self.y0 - tolerance
            && y <= self.y1 + tolerance
    }

    fn area(&self) -> f64 {
        (self.x1 - self.x0) * (self.y1 - self.y0)
    }
}

impl Geometry {
    fn get(&self, name: &str) -> Option<f64> {
        self.cells.get(name).copied()
    }

    /// A point in the shape's own coordinates, in its parent's: the
    /// shape's pin is where its local pin sits, turned by its angle.
    fn place(&self, (x, y): Point) -> Option<Point> {
        let (px, py) = (self.get("PinX")?, self.get("PinY")?);
        let w = self.get("Width").unwrap_or(0.0).abs();
        let h = self.get("Height").unwrap_or(0.0).abs();
        let lx = self.get("LocPinX").unwrap_or(w / 2.0);
        let ly = self.get("LocPinY").unwrap_or(h / 2.0);
        let (sin, cos) = self.get("Angle").unwrap_or(0.0).sin_cos();
        let (dx, dy) = (x - lx, y - ly);
        Some((px + dx * cos - dy * sin, py + dx * sin + dy * cos))
    }

    /// The end points of a 1-D shape (line or connector): those of its
    /// drawn path when known (some writers store only the bounding box in
    /// the begin and end cells), otherwise the begin and end cells.
    pub(crate) fn ends(&self) -> Option<(Point, Point)> {
        let cells = (
            (self.get("BeginX")?, self.get("BeginY")?),
            (self.get("EndX")?, self.get("EndY")?),
        );
        let from_path = || {
            let path = self.path?;
            Some((self.place(path.first)?, self.place(path.last)?))
        };
        Some(from_path().unwrap_or(cells))
    }

    /// Whether `point` (in the parent's coordinates) lies on or next to a
    /// 2-D shape.
    pub(crate) fn touches(&self, point: Point) -> bool {
        self.corners().is_some_and(|corners| {
            let mut b = Bounds::EMPTY;
            corners.into_iter().for_each(|c| b.add(c));
            b.contains(point, TOUCH_TOLERANCE)
        })
    }

    /// The corners of a 2-D shape in its parent's coordinates.
    fn corners(&self) -> Option<[Point; 4]> {
        let (w, h) = (self.get("Width")?.abs(), self.get("Height")?.abs());
        if w == 0.0 && h == 0.0 {
            return None;
        }
        let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)];
        let placed = corners.map(|c| self.place(c));
        Some([placed[0]?, placed[1]?, placed[2]?, placed[3]?])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(cells: &[(&'static str, f64)]) -> Geometry {
        Geometry {
            cells: cells.iter().copied().collect(),
            path: None,
        }
    }

    fn info(parent: Option<&str>, text: &str, geometry: Geometry) -> ShapeInfo {
        ShapeInfo {
            parent: parent.map(str::to_string),
            visible: !text.is_empty(),
            text: text.into(),
            geometry,
        }
    }

    fn shape(id: &str, text: &str) -> Shape {
        Shape {
            id: id.into(),
            text: text.into(),
            ..Shape::default()
        }
    }

    #[test]
    fn instance_suffixes_are_removed() {
        assert_eq!(strip_instance_suffix("Process.12"), "Process");
        assert_eq!(
            strip_instance_suffix("com.lucidchart.Line"),
            "com.lucidchart.Line"
        );
    }

    #[test]
    fn rotated_bounds_cover_the_shape() {
        let shape = info(
            None,
            "",
            geometry(&[
                ("PinX", 1.0),
                ("PinY", 1.0),
                ("Width", 2.0),
                ("Height", 1.0),
                ("Angle", std::f64::consts::FRAC_PI_2),
            ]),
        );
        let b = page_bounds(&HashMap::new(), &shape).unwrap();
        assert!((b.x0 - 0.5).abs() < 1e-9 && (b.x1 - 1.5).abs() < 1e-9);
        assert!((b.y0 - 0.0).abs() < 1e-9 && (b.y1 - 2.0).abs() < 1e-9);
    }

    #[test]
    fn group_members_are_placed_on_the_page() {
        // A 4 x 2 group pinned at its center (10, 10): its local origin is
        // at (8, 9) on the page.
        let group = geometry(&[
            ("PinX", 10.0),
            ("PinY", 10.0),
            ("Width", 4.0),
            ("Height", 2.0),
        ]);
        let mut map = HashMap::new();
        map.insert("g".to_string(), info(None, "", group));
        assert_eq!(on_page(&map, Some("g"), (1.0, 1.0)), Some((9.0, 10.0)));
    }

    #[test]
    fn connectors_become_glued_or_inferred_connections() {
        let boxed =
            |x: f64| geometry(&[("PinX", x), ("PinY", 0.0), ("Width", 1.0), ("Height", 1.0)]);
        let line = geometry(&[
            ("BeginX", 0.5),
            ("BeginY", 0.0),
            ("EndX", 4.5),
            ("EndY", 0.0),
        ]);
        let mut map = HashMap::new();
        map.insert("1".to_string(), info(None, "A", boxed(0.0)));
        map.insert("2".to_string(), info(None, "B", boxed(5.0)));
        map.insert("3".to_string(), info(None, "", line));
        let shapes = vec![shape("1", "A"), shape("2", "B"), shape("3", "")];
        assert!(
            diagram(shapes.clone(), &map, &Glue::new(), false)
                .unwrap()
                .connections
                .is_empty()
        );
        let d = diagram(shapes.clone(), &map, &Glue::new(), true).unwrap();
        assert_eq!(d.shapes.len(), 2);
        let c = &d.connections[0];
        assert_eq!(
            (c.from.as_str(), c.to.as_str(), c.inferred),
            ("1", "2", true)
        );
        assert_eq!(c.connector, None);

        let mut glue = Glue::new();
        glue.insert("3".into(), (Some("2".into()), Some("1".into())));
        let d = diagram(shapes, &map, &glue, false).unwrap();
        let c = &d.connections[0];
        assert_eq!(
            (c.from.as_str(), c.to.as_str(), c.inferred),
            ("2", "1", false)
        );
    }

    #[test]
    fn connectors_are_labelled_with_their_members_text() {
        // A UML association writes its role and multiplicities in members.
        let mut map = HashMap::new();
        for id in ["order", "customer"] {
            map.insert(id.to_string(), info(None, id, geometry(&[])));
        }
        let line = geometry(&[
            ("BeginX", 0.0),
            ("BeginY", 0.0),
            ("EndX", 1.0),
            ("EndY", 0.0),
        ]);
        map.insert("a".to_string(), info(None, "", line));
        let mut association = shape("a", "");
        association.children = vec![
            shape("a1", "-places"),
            shape("a2", "1"),
            shape("a3", "0..*"),
        ];
        let shapes = vec![
            shape("order", "Order"),
            shape("customer", "Customer"),
            association,
        ];
        let mut glue = Glue::new();
        glue.insert("a".into(), (Some("customer".into()), Some("order".into())));
        let d = diagram(shapes, &map, &glue, false).unwrap();
        let c = &d.connections[0];
        assert_eq!(c.label.as_deref(), Some("-places / 1 / 0..*"));
        // Its members are listed too, so the connector is.
        assert_eq!(c.connector.as_deref(), Some("a"));
    }

    #[test]
    fn branches_start_where_their_trunk_starts() {
        let line = || {
            geometry(&[
                ("BeginX", 0.0),
                ("BeginY", 0.0),
                ("EndX", 1.0),
                ("EndY", 0.0),
            ])
        };
        let mut map = HashMap::new();
        for (id, text) in [("a", "Actor"), ("u1", "Use 1"), ("u2", "Use 2")] {
            map.insert(id.to_string(), info(None, text, geometry(&[])));
        }
        map.insert("t".to_string(), info(None, "", line()));
        map.insert("b".to_string(), info(None, "", line()));
        map.insert("x".to_string(), info(None, "", line()));
        let shapes = ["a", "u1", "u2", "t", "b", "x"]
            .map(|id| shape(id, ""))
            .to_vec();
        let mut glue = Glue::new();
        glue.insert("t".into(), (Some("a".into()), Some("u1".into())));
        glue.insert("b".into(), (Some("t".into()), Some("u2".into())));
        // A connector glued to a missing shape makes no connection.
        glue.insert("x".into(), (Some("gone".into()), Some("u2".into())));
        let d = diagram(shapes, &map, &glue, false).unwrap();
        let ends: Vec<(&str, &str)> = d
            .connections
            .iter()
            .map(|c| (c.from.as_str(), c.to.as_str()))
            .collect();
        assert_eq!(ends, [("a", "u1"), ("a", "u2")]);
    }
}
