//! The document catalog and page annotations: destinations, links,
//! bookmarks, page labels, form fields and attached files.

use std::collections::{BTreeMap, HashMap, HashSet};

use doctopus_core::{
    Block, Inline, Link, ListItem, Part, PartKind, Table, TableCell, TableRow, format_number,
};
use hayro_syntax::Pdf;
use hayro_syntax::object::{
    Array, Dict, Name, Object, ObjectIdentifier, Rect, Stream, String as PdfString,
};

use crate::decode::{self, Problem};
use crate::strings;

/// Trees deeper than this are cut off.
const MAX_DEPTH: usize = 32;
/// Entries read from one name tree, outline or field tree at most.
const MAX_ENTRIES: usize = 100_000;
/// Bookmarks nested deeper than this are shown at this level.
const MAX_BOOKMARK_LEVEL: u8 = 8;

/// The parts of the catalog that need resolving more than once.
#[derive(Debug)]
pub(crate) struct Catalog<'a> {
    root: Option<Dict<'a>>,
    /// Page numbers by the object id of the page.
    pages: HashMap<ObjectIdentifier, u32>,
    /// Page numbers of named destinations.
    named: BTreeMap<Vec<u8>, u32>,
}

/// A link annotation: where it is on the page and where it goes.
#[derive(Debug)]
pub(crate) struct PageLink {
    pub(crate) areas: Vec<kurbo::Rect>,
    pub(crate) url: String,
}

/// The annotations of one page that doctopus reads.
#[derive(Debug, Default)]
pub(crate) struct Annotations<'a> {
    pub(crate) links: Vec<PageLink>,
    /// File specifications of file attachment annotations.
    pub(crate) files: Vec<Dict<'a>>,
}

impl<'a> Catalog<'a> {
    pub(crate) fn new(pdf: &'a Pdf) -> Self {
        let xref = pdf.xref();
        let root = xref.get::<Dict<'a>>(xref.root_id());
        let pages = pdf
            .pages()
            .iter()
            .enumerate()
            .filter_map(|(i, page)| Some((page.raw().obj_id()?, i as u32 + 1)))
            .collect();
        let mut catalog = Self {
            root,
            pages,
            named: BTreeMap::new(),
        };
        let mut named = BTreeMap::new();
        if let Some(root) = &catalog.root {
            if let Some(dests) = root.get::<Dict<'a>>(b"Dests") {
                for key in dests.keys().take(MAX_ENTRIES) {
                    if let Some(page) = dests
                        .get::<Object<'a>>(&*key)
                        .and_then(|v| catalog.dest_page(v, 0))
                    {
                        named.insert(key.to_vec(), page);
                    }
                }
            }
            if let Some(tree) = root
                .get::<Dict<'a>>(b"Names")
                .and_then(|n| n.get::<Dict<'a>>(b"Dests"))
            {
                for (key, value) in name_tree(&tree) {
                    if let Some(page) = catalog.dest_page(value, 0) {
                        named.entry(key).or_insert(page);
                    }
                }
            }
        }
        catalog.named = named;
        catalog
    }

    /// The page a destination points to.
    fn dest_page(&self, dest: Object<'a>, depth: usize) -> Option<u32> {
        if depth > 2 {
            return None;
        }
        match dest {
            Object::Array(array) => match array.iter::<Object<'a>>().next()? {
                Object::Dict(page) => self.pages.get(&page.obj_id()?).copied(),
                Object::Number(n) => u32::try_from(n.as_i64()).ok().map(|n| n + 1),
                _ => None,
            },
            Object::Name(name) => self.named.get(&*name).copied(),
            Object::String(name) => self.named.get(name.as_bytes()).copied(),
            Object::Dict(dict) => self.dest_page(dict.get::<Object<'a>>(b"D")?, depth + 1),
            _ => None,
        }
    }

    /// Where an action leads: a web address, a page of this document, or
    /// another file.
    fn action_url(&self, action: &Dict<'a>) -> Option<String> {
        let kind = action.get::<Name<'_>>(b"S")?;
        match kind.as_ref() {
            b"URI" => {
                let uri = action.get::<PdfString<'_>>(b"URI")?;
                let uri = String::from_utf8_lossy(uri.as_bytes()).trim().to_string();
                (!uri.is_empty()).then_some(uri)
            }
            b"GoTo" => Some(page_url(
                self.dest_page(action.get::<Object<'a>>(b"D")?, 0)?,
            )),
            b"GoToR" | b"Launch" => file_spec_name(action.get::<Object<'a>>(b"F")?),
            _ => None,
        }
    }

    /// Where a link annotation or bookmark leads.
    fn target(&self, dict: &Dict<'a>) -> Option<String> {
        if let Some(action) = dict.get::<Dict<'a>>(b"A") {
            return self.action_url(&action);
        }
        dict.get::<Object<'a>>(b"Dest")
            .and_then(|dest| self.dest_page(dest, 0))
            .map(page_url)
    }

    /// The links and file attachments of a page.
    pub(crate) fn annotations(&self, page: &Dict<'a>) -> Annotations<'a> {
        let mut out = Annotations::default();
        let Some(annots) = page.get::<Array<'a>>(b"Annots") else {
            return out;
        };
        for annot in annots.iter::<Dict<'a>>().take(MAX_ENTRIES) {
            let Some(kind) = annot.get::<Name<'_>>(b"Subtype") else {
                continue;
            };
            match kind.as_ref() {
                b"Link" => {
                    let Some(url) = self.target(&annot) else {
                        continue;
                    };
                    let areas = link_areas(&annot);
                    if !areas.is_empty() {
                        out.links.push(PageLink { areas, url });
                    }
                }
                b"FileAttachment" => {
                    if let Some(spec) = annot.get::<Dict<'a>>(b"FS") {
                        out.files.push(spec);
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The bookmarks as a nested list of links to pages.
    pub(crate) fn outline(&self) -> Vec<Block> {
        let mut out = Vec::new();
        let first = self
            .root
            .as_ref()
            .and_then(|r| r.get::<Dict<'a>>(b"Outlines"))
            .and_then(|o| o.get::<Dict<'a>>(b"First"));
        let mut seen = HashSet::new();
        self.outline_items(first, 0, &mut seen, &mut out);
        out
    }

    fn outline_items(
        &self,
        mut item: Option<Dict<'a>>,
        depth: usize,
        seen: &mut HashSet<ObjectIdentifier>,
        out: &mut Vec<Block>,
    ) {
        if depth > MAX_DEPTH {
            return;
        }
        while let Some(dict) = item {
            if out.len() >= MAX_ENTRIES || dict.obj_id().is_some_and(|id| !seen.insert(id)) {
                return;
            }
            let title = dict
                .get::<PdfString<'_>>(b"Title")
                .map(|t| one_line(&strings::text(t.as_bytes())))
                .unwrap_or_default();
            if !title.is_empty() {
                let content = match self.target(&dict) {
                    Some(url) => vec![Inline::Link(Link {
                        url,
                        content: vec![Inline::text(title)],
                    })],
                    None => vec![Inline::text(title)],
                };
                out.push(Block::ListItem(ListItem {
                    level: (depth as u8).min(MAX_BOOKMARK_LEVEL),
                    label: None,
                    content,
                }));
            }
            self.outline_items(dict.get::<Dict<'a>>(b"First"), depth + 1, seen, out);
            item = dict.get::<Dict<'a>>(b"Next");
        }
    }

    /// The label of each page (`iv`, `A-3`) where page labels are defined.
    pub(crate) fn page_labels(&self, count: usize) -> Vec<Option<String>> {
        let mut labels = vec![None; count];
        let Some(tree) = self
            .root
            .as_ref()
            .and_then(|r| r.get::<Dict<'a>>(b"PageLabels"))
        else {
            return labels;
        };
        let mut ranges: Vec<(usize, Dict<'a>)> = number_tree(&tree)
            .into_iter()
            .filter_map(|(start, value)| match value {
                Object::Dict(d) => Some((usize::try_from(start).ok()?, d)),
                _ => None,
            })
            .collect();
        ranges.sort_by_key(|(start, _)| *start);
        for (n, (start, style)) in ranges.iter().enumerate() {
            let end = ranges
                .get(n + 1)
                .map_or(count, |(next, _)| *next)
                .min(count);
            let prefix = style
                .get::<PdfString<'_>>(b"P")
                .map(|p| strings::text(p.as_bytes()))
                .unwrap_or_default();
            let first = style.get::<i64>(b"St").unwrap_or(1).max(1);
            let kind = style.get::<Name<'_>>(b"S");
            for (offset, label) in labels.iter_mut().enumerate().take(end).skip(*start) {
                let value = first.saturating_add((offset - start) as i64);
                let numeral = match kind.as_deref() {
                    Some(b"D") => value.to_string(),
                    Some(b"R") => roman(value),
                    Some(b"r") => roman(value).to_lowercase(),
                    Some(b"A") => letters(value),
                    Some(b"a") => letters(value).to_lowercase(),
                    _ => String::new(),
                };
                let text = format!("{prefix}{numeral}");
                if !text.is_empty() && text != (offset + 1).to_string() {
                    *label = Some(text);
                }
            }
        }
        labels
    }

    /// The interactive form fields with their values, as a table.
    pub(crate) fn form_fields(&self) -> Option<Table> {
        let fields = self
            .root
            .as_ref()?
            .get::<Dict<'a>>(b"AcroForm")?
            .get::<Array<'a>>(b"Fields")?;
        let mut rows = vec![row(["Field", "Type", "Value"])];
        let mut seen = HashSet::new();
        for field in fields.iter::<Dict<'a>>() {
            collect_fields(&field, &Inherited::default(), 0, &mut seen, &mut rows);
        }
        (rows.len() > 1).then_some(Table {
            rows,
            header_rows: 1,
        })
    }

    /// The files attached to the document as a whole, with their names in
    /// the `EmbeddedFiles` name tree.
    pub(crate) fn embedded_files(&self) -> Vec<(String, Dict<'a>)> {
        let Some(tree) = self
            .root
            .as_ref()
            .and_then(|r| r.get::<Dict<'a>>(b"Names"))
            .and_then(|n| n.get::<Dict<'a>>(b"EmbeddedFiles"))
        else {
            return Vec::new();
        };
        name_tree(&tree)
            .into_iter()
            .filter_map(|(key, value)| match value {
                Object::Dict(spec) => Some((strings::text(&key), spec)),
                _ => None,
            })
            .collect()
    }

    /// The document's language, as declared in the catalog.
    pub(crate) fn language(&self) -> Option<String> {
        let lang = self.root.as_ref()?.get::<PdfString<'_>>(b"Lang")?;
        Some(strings::text(lang.as_bytes()))
    }

    /// The XMP metadata stream.
    pub(crate) fn metadata(&self) -> Option<Stream<'a>> {
        self.root.as_ref()?.get::<Stream<'a>>(b"Metadata")
    }
}

fn page_url(page: u32) -> String {
    format!("#page-{page}")
}

/// The areas a link covers: its quadrilaterals when given (a link over
/// several lines), its rectangle otherwise.
fn link_areas(annot: &Dict<'_>) -> Vec<kurbo::Rect> {
    let quads: Vec<f64> = annot
        .get::<Array<'_>>(b"QuadPoints")
        .map(|q| q.iter::<f64>().take(8 * 1024).collect())
        .unwrap_or_default();
    if quads.len() >= 8 {
        let (quads, _) = quads.as_chunks::<8>();
        return quads
            .iter()
            .map(|q| {
                let xs = [q[0], q[2], q[4], q[6]];
                let ys = [q[1], q[3], q[5], q[7]];
                let (x0, x1) = (
                    xs.iter().copied().fold(f64::MAX, f64::min),
                    xs.iter().copied().fold(f64::MIN, f64::max),
                );
                let (y0, y1) = (
                    ys.iter().copied().fold(f64::MAX, f64::min),
                    ys.iter().copied().fold(f64::MIN, f64::max),
                );
                kurbo::Rect::new(x0, y0, x1, y1)
            })
            .collect();
    }
    annot
        .get::<Rect>(b"Rect")
        .map(|r| vec![kurbo::Rect::new(r.x0, r.y0, r.x1, r.y1).abs()])
        .unwrap_or_default()
}

/// The file name of a file specification: a string or a dictionary.
fn file_spec_name(spec: Object<'_>) -> Option<String> {
    let name = match spec {
        Object::String(s) => strings::text(s.as_bytes()),
        Object::Dict(d) => spec_name(&d)?,
        _ => return None,
    };
    (!name.trim().is_empty()).then(|| name.trim().to_string())
}

/// The name a file specification dictionary gives its file.
fn spec_name(spec: &Dict<'_>) -> Option<String> {
    [&b"UF"[..], b"F", b"Unix", b"DOS", b"Mac"]
        .iter()
        .find_map(|key| spec.get::<PdfString<'_>>(*key))
        .map(|s| strings::text(s.as_bytes()))
}

/// The entries of a name tree, in tree order.
fn name_tree<'a>(root: &Dict<'a>) -> Vec<(Vec<u8>, Object<'a>)> {
    let mut out = Vec::new();
    walk_tree(root, b"Names", 0, &mut HashSet::new(), &mut |key, value| {
        if let Object::String(key) = key {
            out.push((key.as_bytes().to_vec(), value));
        }
    });
    out
}

/// The entries of a number tree, in tree order.
fn number_tree<'a>(root: &Dict<'a>) -> Vec<(i64, Object<'a>)> {
    let mut out = Vec::new();
    walk_tree(root, b"Nums", 0, &mut HashSet::new(), &mut |key, value| {
        if let Object::Number(n) = key {
            out.push((n.as_i64(), value));
        }
    });
    out
}

/// Visit the key-value pairs of a name or number tree.
fn walk_tree<'a>(
    node: &Dict<'a>,
    entries: &[u8],
    depth: usize,
    seen: &mut HashSet<ObjectIdentifier>,
    visit: &mut dyn FnMut(Object<'a>, Object<'a>),
) {
    if depth > MAX_DEPTH
        || seen.len() >= MAX_ENTRIES
        || node.obj_id().is_some_and(|id| !seen.insert(id))
    {
        return;
    }
    if let Some(pairs) = node.get::<Array<'a>>(entries) {
        let mut items = pairs.iter::<Object<'a>>().take(2 * MAX_ENTRIES);
        while let (Some(key), Some(value)) = (items.next(), items.next()) {
            visit(key, value);
        }
    }
    if let Some(kids) = node.get::<Array<'a>>(b"Kids") {
        for kid in kids.iter::<Dict<'a>>() {
            walk_tree(&kid, entries, depth + 1, seen, visit);
        }
    }
}

/// Field attributes passed down from parent fields.
#[derive(Debug, Default, Clone)]
struct Inherited {
    name: String,
    kind: Option<Vec<u8>>,
    flags: i64,
    value: Option<String>,
}

fn collect_fields(
    field: &Dict<'_>,
    parent: &Inherited,
    depth: usize,
    seen: &mut HashSet<ObjectIdentifier>,
    rows: &mut Vec<TableRow>,
) {
    if depth > MAX_DEPTH
        || rows.len() > MAX_ENTRIES
        || field.obj_id().is_some_and(|id| !seen.insert(id))
    {
        return;
    }
    let mut this = parent.clone();
    if let Some(partial) = field.get::<PdfString<'_>>(b"T") {
        let partial = strings::text(partial.as_bytes());
        this.name = if parent.name.is_empty() {
            partial
        } else {
            format!("{}.{partial}", parent.name)
        };
    }
    if let Some(kind) = field.get::<Name<'_>>(b"FT") {
        this.kind = Some(kind.to_vec());
    }
    if let Some(flags) = field.get::<i64>(b"Ff") {
        this.flags = flags;
    }
    if let Some(value) = field.get::<Object<'_>>(b"V") {
        this.value = Some(field_value(value));
    }
    // Kids without names are the field's widgets, not fields of their own.
    let kids: Vec<Dict<'_>> = field
        .get::<Array<'_>>(b"Kids")
        .map(|k| {
            k.iter::<Dict<'_>>()
                .filter(|kid| kid.contains_key(b"T"))
                .collect()
        })
        .unwrap_or_default();
    if !kids.is_empty() {
        for kid in kids {
            collect_fields(&kid, &this, depth + 1, seen, rows);
        }
        return;
    }
    let kind = match (this.kind.as_deref(), this.flags) {
        (Some(b"Btn"), f) if f & (1 << 16) != 0 => return,
        (Some(b"Btn"), f) if f & (1 << 15) != 0 => "radio button",
        (Some(b"Btn"), _) => "check box",
        (Some(b"Tx"), _) => "text",
        (Some(b"Ch"), f) if f & (1 << 17) != 0 => "combo box",
        (Some(b"Ch"), _) => "list box",
        (Some(b"Sig"), _) => "signature",
        _ => "field",
    };
    if this.name.is_empty() {
        return;
    }
    let value = match (this.value.as_deref(), kind) {
        (Some("Off") | None, "check box" | "radio button") => "",
        (Some(value), _) => value,
        (None, _) => "",
    };
    rows.push(row([this.name.as_str(), kind, value]));
}

fn field_value(value: Object<'_>) -> String {
    match value {
        Object::String(s) => strings::text(s.as_bytes()),
        Object::Name(n) => n.as_str().to_string(),
        Object::Number(n) => format_number(n.as_f64()),
        Object::Boolean(b) => b.to_string(),
        Object::Array(items) => items
            .iter::<Object<'_>>()
            .take(1000)
            .map(field_value)
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join("; "),
        _ => String::new(),
    }
}

fn row<'s>(cells: impl IntoIterator<Item = &'s str>) -> TableRow {
    TableRow {
        cells: cells
            .into_iter()
            .map(|text| {
                TableCell::new(if text.is_empty() {
                    Vec::new()
                } else {
                    vec![Block::text(text)]
                })
            })
            .collect(),
    }
}

/// The file behind a file specification, as a part.
pub(crate) fn attachment(spec: &Dict<'_>, key: Option<&str>, limit: u64) -> Result<Part, String> {
    let name = spec_name(spec)
        .or_else(|| key.map(str::to_string))
        .map(|n| strings::file_name(&n).to_string())
        .filter(|n| !n.is_empty());
    let label = name.clone().unwrap_or_else(|| "an attached file".into());
    let stream = spec
        .get::<Dict<'_>>(b"EF")
        .and_then(|ef| {
            ef.get::<Stream<'_>>(b"UF")
                .or_else(|| ef.get::<Stream<'_>>(b"F"))
        })
        .ok_or_else(|| format!("{label} is referred to but not embedded"))?;
    let data = decode::decoded(&stream, limit).map_err(|problem| match problem {
        Problem::TooLarge => format!(
            "{label} is larger than {} MiB and was not extracted",
            limit >> 20
        ),
        Problem::Damaged => format!("{label} could not be decoded"),
    })?;
    let mut part = Part::new(PartKind::File, data);
    if let Some(name) = name {
        part = part.with_name(name);
    }
    if let Some(desc) = spec.get::<PdfString<'_>>(b"Desc") {
        let desc = one_line(&strings::text(desc.as_bytes()));
        if !desc.is_empty() {
            part = part.with_description(desc);
        }
    }
    if let Some(mime) = stream.dict().get::<Name<'_>>(b"Subtype")
        && mime.as_str().contains('/')
    {
        part = part.with_content_type(mime.as_str().to_ascii_lowercase());
    }
    if let Some(id) = stream.dict().obj_id() {
        part = part.with_origin(origin(id));
    }
    Ok(part)
}

/// A stable name for a PDF object, used to store a shared object once.
pub(crate) fn origin(id: ObjectIdentifier) -> String {
    match id.gen_number {
        0 => format!("object {}", id.obj_number),
        generation => format!("object {} {generation}", id.obj_number),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Upper-case roman numerals; plain digits beyond 3999.
fn roman(mut n: i64) -> String {
    if !(1..=3999).contains(&n) {
        return n.to_string();
    }
    const NUMERALS: [(i64, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, numeral) in NUMERALS {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    out
}

/// Letter labels: A to Z, then AA to ZZ, and so on.
fn letters(n: i64) -> String {
    if !(1..=26 * 64).contains(&n) {
        return n.to_string();
    }
    let letter = char::from(b'A' + ((n - 1) % 26) as u8);
    std::iter::repeat_n(letter, ((n - 1) / 26 + 1) as usize).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numerals_follow_the_page_label_styles() {
        assert_eq!(roman(1994), "MCMXCIV");
        assert_eq!(roman(4), "IV");
        assert_eq!(roman(0), "0");
        assert_eq!(letters(1), "A");
        assert_eq!(letters(28), "BB");
        assert_eq!(letters(0), "0");
    }
}
