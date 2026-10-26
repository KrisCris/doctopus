//! One shape as stored: what the shape sets itself, before what it
//! inherits from its master is merged in.

use std::collections::HashMap;

use super::kind;
use super::stream::Version;
use super::tree::Node;
use crate::binary::{Cursor, i16_at, i32_at, u16_at, u32_at};
use crate::dates::civil_from_days;
use crate::diagram::{GEOMETRY, format_number};
use crate::ole::text::{ansi, utf16le};

/// Marks a field in version 6 (and 5) text; version 11 text uses U+FFFC.
const FIELD_MARK: u8 = 0x1E;
const FIELD_MARK_UNICODE: char = '\u{FFFC}';
/// Bytes after a version 5 field mark that describe the field.
const V5_FIELD_INFO: usize = 3;

/// The cell type of a string (a name) rather than a number.
const CELL_STRING: u8 = 0xE8;
/// The cell type of a value with a unit raised to a power (an area).
const CELL_DIMENSIONED: u8 = 0xE9;
/// The cell type of a date.
const CELL_DATE: u8 = 0x28;
/// The cell type of feet and inches.
const CELL_FEET_INCHES: u8 = 0x43;
/// The cell type of an angle shown in degrees, minutes and seconds.
const CELL_DEGREES_MINUTES_SECONDS: u8 = 0x52;
/// Formula token of a number with a unit (followed by the number, the
/// unit and its power).
const TOKEN_DIMENSIONED: u8 = 0x46;
/// Formula token of a string (followed by its length in characters and
/// the characters).
const TOKEN_STRING: u8 = 0x60;

/// A string id meaning the string is the literal formula of its cell.
const IN_FORMULA: i32 = -3;
/// Bytes of a shape data row before its formulas, at least.
const PROP_FIXED: usize = 26;
/// Cells of a shape data row, as formula blocks number them.
const PROP_VALUE: u8 = 0;
const PROP_LABEL: u8 = 2;
const PROP_FORMAT: u8 = 3;
/// Property types: a string, a list to choose from, a boolean, a list
/// that also takes other values.
const PROP_STRING: u8 = 0;
const PROP_FIXED_LIST: u8 = 1;
const PROP_BOOLEAN: u8 = 3;
const PROP_VARIABLE_LIST: u8 = 4;
/// Bytes of a hyperlink row before its formulas, at least.
const LINK_FIXED: usize = 20;
/// Cells of a hyperlink row, as formula blocks number them.
const LINK_ADDRESS: u8 = 1;
const LINK_SUB_ADDRESS: u8 = 2;

/// Days from 1899-12-30 (day 0 of Visio dates) to 1970-01-01.
const DAYS_TO_UNIX_EPOCH: i64 = 25_569;

/// Shape types.
pub(super) fn is_shape(node: &Node) -> bool {
    matches!(
        node.kind,
        kind::SHAPE_GROUP | kind::SHAPE_SHAPE | kind::SHAPE_GUIDE | kind::SHAPE_FOREIGN
    )
}

#[derive(Debug, Clone)]
pub(super) enum TextPiece {
    Text(String),
    /// The place of the next field.
    Field,
}

#[derive(Debug, Clone)]
pub(super) enum Field {
    /// A string, by its id in the shape's names.
    Name(i32),
    Number(Number),
}

/// A numeric field.
#[derive(Debug, Clone)]
pub(super) struct Number {
    pub(super) value: f64,
    /// The unit, as a cell type.
    pub(super) cell: u8,
    /// 2 for an area, 3 for a volume.
    pub(super) power: u8,
    /// The built-in display format.
    pub(super) format: Option<u16>,
    /// The display format, when given as text: `esc(N)` for built-in
    /// format N, or a picture such as `{{dd.MM.yyyy}}` or `#,##0.00 u`.
    pub(super) picture: Option<TextRef>,
}

/// Where a string is.
#[derive(Debug, Clone)]
pub(super) enum TextRef {
    /// A string in the shape's names, by its id.
    Name(i32),
    /// In the cell's formula.
    Text(String),
}

impl TextRef {
    /// The string itself, looked up in `names` when it is there.
    fn resolve(&self, names: &HashMap<i32, String>) -> TextRef {
        match self {
            TextRef::Name(id) => TextRef::Text(names.get(id).cloned().unwrap_or_default()),
            text => text.clone(),
        }
    }

    /// The text of a resolved string.
    fn text(&self) -> &str {
        match self {
            TextRef::Text(t) => t,
            TextRef::Name(_) => "",
        }
    }
}

/// A shape data row (a custom property), as far as the shape sets it.
#[derive(Debug, Clone)]
pub(super) struct Prop {
    /// The row: a shape's row overrides its master's row of the same id.
    pub(super) id: u32,
    pub(super) value: PropValue,
    /// `None` when the row takes its master's.
    pub(super) label: Option<TextRef>,
    /// A picture for numbers, or the choices of a list.
    pub(super) format: Option<TextRef>,
    /// The property type: string, list, number, boolean, date...
    pub(super) kind: u8,
}

#[derive(Debug, Clone)]
pub(super) enum PropValue {
    Number { value: f64, cell: u8 },
    Text(Option<TextRef>),
}

impl Prop {
    /// The row with its strings looked up in `names`.
    fn resolved(&self, names: &HashMap<i32, String>) -> Prop {
        let resolve = |t: &Option<TextRef>| t.as_ref().map(|t| t.resolve(names));
        Prop {
            value: match &self.value {
                PropValue::Text(t) => PropValue::Text(resolve(t)),
                number => number.clone(),
            },
            label: resolve(&self.label),
            format: resolve(&self.format),
            ..self.clone()
        }
    }
}

/// A hyperlink row.
#[derive(Debug, Clone)]
pub(super) struct Link {
    pub(super) id: u32,
    pub(super) address: Option<TextRef>,
    pub(super) sub_address: Option<TextRef>,
}

/// A picture or embedded object shown by a shape.
#[derive(Debug, Clone, Copy)]
pub(super) struct Foreign<'n> {
    /// 0 and 4: metafile, 1: bitmap, 2: OLE object.
    pub(super) kind: u16,
    /// For bitmaps: 0 BMP, 1 JPEG, 2 GIF, 3 TIFF, 4 PNG.
    pub(super) format: u32,
    /// The picture (for an object, its preview).
    pub(super) data: Option<&'n Node>,
    /// The object's data, split over several chunks.
    pub(super) object: Option<&'n Node>,
}

pub(super) const FOREIGN_OBJECT: u16 = 2;
pub(super) const FOREIGN_BITMAP: u16 = 1;
pub(super) const FOREIGN_EMF: u16 = 4;
/// The map mode that makes a metafile an enhanced one.
const MAP_MODE_EMF: u16 = 8;

/// A shape as stored.
#[derive(Debug)]
pub(super) struct ShapeDef<'n> {
    pub(super) id: u32,
    pub(super) kind: u32,
    /// The group the shape belongs to.
    pub(super) parent: Option<u32>,
    /// The stencil page and shape the shape was made from.
    pub(super) master: Option<u32>,
    pub(super) master_shape: Option<u32>,
    pub(super) text: Option<Vec<TextPiece>>,
    pub(super) fields: Option<Vec<Field>>,
    /// Strings that fields refer to, by id.
    pub(super) names: HashMap<i32, String>,
    pub(super) cells: HashMap<&'static str, f64>,
    /// Shapes the ends of a connector are glued to, in the order found.
    pub(super) glue: Vec<u32>,
    pub(super) foreign: Option<Foreign<'n>>,
    /// Shape data rows (not read in version 5).
    pub(super) props: Vec<Prop>,
    /// Hyperlink rows (not read in version 5).
    pub(super) links: Vec<Link>,
}

impl<'n> ShapeDef<'n> {
    pub(super) fn read(version: Version, node: &'n Node) -> ShapeDef<'n> {
        let data = node.data();
        let ids = |at: [usize; 3]| -> [Option<u32>; 3] {
            at.map(|pos| match version {
                Version::V5 => u16_at(data, pos).filter(|&v| v != u16::MAX).map(u32::from),
                Version::V6 | Version::V11 => u32_at(data, pos).filter(|&v| v != u32::MAX),
            })
        };
        let [parent, master, master_shape] = match version {
            Version::V5 => ids([2, 6, 8]),
            Version::V6 | Version::V11 => ids([10, 18, 26]),
        };
        let find = |k: u32| node.find(k, &is_shape);
        let mut def = ShapeDef {
            id: node.id,
            kind: node.kind,
            parent: parent.filter(|&p| p != 0),
            master,
            master_shape,
            text: find(kind::TEXT).map(|t| text(version, t.data())),
            fields: find(kind::FIELD_LIST).map(|list| {
                let members = match version {
                    // Version 5 lists keep their members in order.
                    Version::V5 => list.children.iter().collect(),
                    Version::V6 | Version::V11 => list.ordered_children(),
                };
                members
                    .into_iter()
                    .filter(|f| f.kind == kind::TEXT_FIELD)
                    .filter_map(|f| field(version, f))
                    .collect()
            }),
            names: HashMap::new(),
            cells: HashMap::new(),
            glue: Vec::new(),
            foreign: None,
            props: Vec::new(),
            links: Vec::new(),
        };
        if let Some(list) = find(kind::NAME_LIST) {
            for name in list.children_of(kind::NAME) {
                let data = name.data();
                let text = match version {
                    Version::V11 => utf16le(data),
                    Version::V5 | Version::V6 => zstring(data),
                };
                def.names.insert(name.id as i32, text);
            }
        }
        if let Some(xform) = find(kind::XFORM) {
            def.cells.extend(doubles(xform.data(), &GEOMETRY[..7]));
        }
        if let Some(xform) = find(kind::XFORM_1D) {
            def.cells.extend(doubles(xform.data(), &GEOMETRY[7..]));
        }
        if let Some(misc) = find(kind::MISC) {
            def.glue = glue(version, misc.extended());
        }
        if version != Version::V5 {
            let mut rows = Vec::new();
            let row = |n: &Node| matches!(n.kind, kind::PROP | kind::HYPERLINK);
            node.collect(&row, &is_shape, &mut rows);
            for row in rows {
                if row.kind == kind::PROP {
                    def.props.extend(prop(version, row));
                } else {
                    def.links.extend(link(version, row));
                }
            }
        }
        if let Some(info) = find(kind::FOREIGN_TYPE) {
            let d = info.data();
            let mut kind = u16_at(d, 36).unwrap_or(0);
            if u16_at(d, 38) == Some(MAP_MODE_EMF) {
                kind = FOREIGN_EMF;
            }
            def.foreign = Some(Foreign {
                kind,
                format: u32_at(d, 49).unwrap_or(0),
                data: find(kind::FOREIGN_DATA),
                object: find(kind::OLE_LIST),
            });
        }
        def
    }
}

/// A shape data row: the value cell (a string id for strings, a number
/// otherwise), then the ids of the prompt, label, format and sort key
/// strings, then the type; the formulas of the row's cells follow.
fn prop(version: Version, node: &Node) -> Option<Prop> {
    let data = node.data();
    let strings = formula_strings(version, data, PROP_FIXED);
    let text = |at: usize, cell: u8| text_ref(i32_at(data, at)?, cell, &strings);
    let cell = *data.first()?;
    let value = if cell == CELL_STRING {
        PropValue::Text(text(1, PROP_VALUE))
    } else {
        PropValue::Number {
            value: Cursor::at(data, 1).f64()?,
            cell,
        }
    };
    Some(Prop {
        id: node.id,
        value,
        label: text(13, PROP_LABEL),
        format: text(17, PROP_FORMAT),
        kind: *data.get(25)?,
    })
}

/// A hyperlink row: the ids of its description, address and sub-address
/// strings come first.
fn link(version: Version, node: &Node) -> Option<Link> {
    let data = node.data();
    let strings = formula_strings(version, data, LINK_FIXED);
    let text = |at: usize, cell: u8| text_ref(i32_at(data, at)?, cell, &strings);
    Some(Link {
        id: node.id,
        address: text(4, LINK_ADDRESS),
        sub_address: text(8, LINK_SUB_ADDRESS),
    })
}

fn text_ref(id: i32, cell: u8, strings: &HashMap<u8, String>) -> Option<TextRef> {
    match id {
        IN_FORMULA => strings.get(&cell).cloned().map(TextRef::Text),
        id if id >= 0 => Some(TextRef::Name(id)),
        _ => None,
    }
}

/// The string literals among the formulas of a row's cells, by cell. The
/// formulas follow the row's fixed part (at least `from` bytes) as
/// blocks: the block length (u32), 2, the cell, then tokens ending with
/// 0xFE. A string is 0x60, its length in characters, the characters
/// (UTF-16 in version 11) and a terminator.
fn formula_strings(version: Version, data: &[u8], from: usize) -> HashMap<u8, String> {
    let mut out = HashMap::new();
    let Some(mut at) = (from..data.len()).find(|&at| formula_chain(data, at)) else {
        return out;
    };
    while let Some(length) = u32_at(data, at).map(|l| l as usize).filter(|&l| l > 0) {
        if data.get(at + 6) == Some(&TOKEN_STRING)
            && let (Some(&cell), Some(&chars)) = (data.get(at + 5), data.get(at + 7))
        {
            let (start, chars) = (at + 8, usize::from(chars));
            let text = match version {
                Version::V11 => data.get(start..start + 2 * chars).map(utf16le),
                Version::V5 | Version::V6 => data.get(start..start + chars).map(ansi),
            };
            out.extend(text.map(|t| (cell, t)));
        }
        at += length;
    }
    out
}

/// Whether formula blocks start at `at` and run to the end of the data
/// (or to a zero length).
fn formula_chain(data: &[u8], mut at: usize) -> bool {
    let mut blocks = 0;
    loop {
        match u32_at(data, at).map(|l| l as usize) {
            Some(0) => return blocks > 0,
            None => return blocks > 0 && at == data.len(),
            Some(length) => {
                let end = at.saturating_add(length);
                if length < 7 || end > data.len() || data[at + 4] != 2 || data[end - 1] != 0xFE {
                    return false;
                }
                at = end;
                blocks += 1;
            }
        }
    }
}

/// Text split at its fields. The first 8 bytes are not text.
fn text(version: Version, data: &[u8]) -> Vec<TextPiece> {
    let data = data.get(8..).unwrap_or_default();
    let mut pieces = Vec::new();
    match version {
        Version::V11 => {
            for (i, part) in utf16le(data).split(FIELD_MARK_UNICODE).enumerate() {
                if i > 0 {
                    pieces.push(TextPiece::Field);
                }
                pieces.push(TextPiece::Text(part.to_string()));
            }
        }
        Version::V5 | Version::V6 => {
            let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
            for (i, mut part) in data[..end].split(|&b| b == FIELD_MARK).enumerate() {
                if i > 0 {
                    pieces.push(TextPiece::Field);
                    if version == Version::V5 {
                        part = part.get(V5_FIELD_INFO..).unwrap_or_default();
                    }
                }
                pieces.push(TextPiece::Text(ansi(part)));
            }
        }
    }
    pieces
}

fn field(version: Version, node: &Node) -> Option<Field> {
    let data = node.data();
    let (cell_at, value_at) = match version {
        Version::V5 => (3, 4),
        Version::V6 | Version::V11 => (7, 8),
    };
    let cell = *data.get(cell_at)?;
    if cell == CELL_STRING {
        let id = match version {
            Version::V5 => i32::from(i16_at(data, value_at)?),
            Version::V6 | Version::V11 => i32_at(data, value_at)?,
        };
        return Some(Field::Name(id));
    }
    // The formula of the field follows its fixed part.
    let formula = node.extended().get(value_at + 8..).unwrap_or_default();
    let mut number = Number {
        value: Cursor::at(data, value_at).f64()?,
        cell,
        power: 1,
        format: format_token(formula),
        picture: match version {
            Version::V5 => None,
            Version::V6 => i32_at(data, 18).filter(|&id| id >= 0).map(TextRef::Name),
            Version::V11 => i32_at(data, 18)
                .filter(|&id| id >= 0)
                .map(TextRef::Name)
                .or_else(|| picture_token(formula).map(TextRef::Text)),
        },
    };
    if cell == CELL_DIMENSIONED {
        (number.value, number.cell, number.power) = dimensioned(formula)?;
    }
    Some(Field::Number(number))
}

/// The display format named in a field's formula (a `FORMAT` token).
fn format_token(formula: &[u8]) -> Option<u16> {
    formula
        .windows(7)
        .find(|w| w[0] == 2 && w[1] == 2 && w[5] == 0x80 && w[6] == 0xC2)
        .map(|w| u16::from_le_bytes([w[3], w[4]]))
}

/// A display format given as text in a field's formula.
fn picture_token(formula: &[u8]) -> Option<String> {
    let at = formula.windows(3).position(|w| w == [2, 2, TOKEN_STRING])?;
    let length = usize::from(*formula.get(at + 3)?);
    let text = formula.get(at + 4..at + 4 + 2 * length)?;
    Some(utf16le(text)).filter(|t| !t.trim().is_empty())
}

/// The value, unit and power of a number stored with a unit raised to a
/// power (areas).
fn dimensioned(formula: &[u8]) -> Option<(f64, u8, u8)> {
    formula.windows(11).find_map(|w| {
        let value = f64::from_le_bytes(w[1..9].try_into().ok()?);
        (w[0] == TOKEN_DIMENSIONED
            && matches!(w[10], 2 | 3)
            && !unit(w[9]).1.is_empty()
            && value.is_finite())
        .then_some((value, w[9], w[10]))
    })
}

/// Glue blocks of a connector's miscellaneous cells: each names a shape
/// one end is glued to.
fn glue(version: Version, data: &[u8]) -> Vec<u32> {
    let mut at = match version {
        Version::V5 => return Vec::new(),
        Version::V6 => 23,
        Version::V11 => 45,
    };
    let mut out = Vec::new();
    while let Some(length) = u32_at(data, at).filter(|&l| l != 0) {
        if data.get(at + 4) == Some(&2)
            && data.get(at + 6) == Some(&0x74)
            && u32_at(data, at + 7) == Some(0x6000_004E)
            && data.get(at + 15) == Some(&0x7A)
            && u32_at(data, at + 16) == Some(0x4000_0073)
            && let Some(id) = u32_at(data, at + 11)
        {
            out.push(id);
        }
        let Some(next) = at.checked_add(length as usize) else {
            break;
        };
        at = next;
    }
    out
}

/// Doubles stored with a one-byte prefix each, from offset 1.
fn doubles(data: &[u8], names: &[&'static str]) -> Vec<(&'static str, f64)> {
    names
        .iter()
        .enumerate()
        .filter_map(|(i, &name)| {
            let v = Cursor::at(data, 1 + 9 * i).f64()?;
            v.is_finite().then_some((name, v))
        })
        .collect()
}

fn zstring(data: &[u8]) -> String {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    ansi(&data[..end])
}

// ----- text ------------------------------------------------------------------

/// Text with its fields filled in.
pub(super) fn render_text(
    pieces: &[TextPiece],
    fields: &[Field],
    names: &dyn Fn(i32) -> Option<String>,
) -> String {
    let mut out = String::new();
    let mut next = fields.iter();
    for piece in pieces {
        match piece {
            TextPiece::Text(t) => out.push_str(t),
            TextPiece::Field => match next.next() {
                Some(Field::Name(id)) => out.push_str(&names(*id).unwrap_or_default()),
                Some(Field::Number(number)) => {
                    let picture = match &number.picture {
                        Some(TextRef::Name(id)) => names(*id),
                        Some(TextRef::Text(text)) => Some(text.clone()),
                        None => None,
                    };
                    out.push_str(&format_field(number, picture.as_deref()));
                }
                None => {}
            },
        }
    }
    let out: String = out
        .replace("\r\n", "\n")
        .chars()
        .filter_map(|c| match c {
            '\r' | '\u{2028}' | '\u{2029}' | '\u{000B}' => Some('\n'),
            c if c.is_control() && c != '\n' && c != '\t' => None,
            c => Some(c),
        })
        .collect();
    out.trim().to_string()
}

/// A shape's data, label and value, over its master's: rows the shape
/// sets override the master's rows of the same id. Rows without a label
/// or a value are left out.
pub(super) fn shape_data(def: &ShapeDef<'_>, base: Option<&ShapeDef<'_>>) -> Vec<(String, String)> {
    // Strings resolved with the names of the shape that sets them.
    let mut rows: Vec<Prop> = base
        .map(|b| b.props.iter().map(|p| p.resolved(&b.names)).collect())
        .unwrap_or_default();
    for prop in def.props.iter().map(|p| p.resolved(&def.names)) {
        match rows.iter_mut().find(|r| r.id == prop.id) {
            Some(row) => {
                row.value = prop.value;
                row.kind = prop.kind;
                row.label = prop.label.or(row.label.take());
                row.format = prop.format.or(row.format.take());
            }
            None => rows.push(prop),
        }
    }
    rows.into_iter()
        .filter_map(|row| {
            let label = without_accelerator(row.label.as_ref().map(TextRef::text)?);
            let value = match &row.value {
                PropValue::Text(text) => text.as_ref().map(TextRef::text)?.to_string(),
                // A string property holds no number: it is empty.
                PropValue::Number { .. } if row.kind == PROP_STRING => return None,
                PropValue::Number { value, .. } if row.kind == PROP_BOOLEAN => {
                    (if *value != 0.0 { "TRUE" } else { "FALSE" }).to_string()
                }
                PropValue::Number { value, cell } => {
                    // The format of a list holds its choices.
                    let list = matches!(row.kind, PROP_FIXED_LIST | PROP_VARIABLE_LIST);
                    let picture = row.format.as_ref().map(TextRef::text).filter(|_| !list);
                    let number = Number {
                        value: *value,
                        cell: *cell,
                        power: 1,
                        format: None,
                        picture: None,
                    };
                    format_field(&number, picture)
                }
            };
            let (label, value) = (label.trim().to_string(), value.trim().to_string());
            (!label.is_empty() && !value.is_empty()).then_some((label, value))
        })
        .collect()
}

/// A shape's hyperlinks over its master's, as `address#sub-address`.
pub(super) fn hyperlinks(def: &ShapeDef<'_>, base: Option<&ShapeDef<'_>>) -> Vec<String> {
    let inherited = base
        .into_iter()
        .flat_map(|b| b.links.iter().map(move |l| (l, &b.names)))
        .filter(|(l, _)| !def.links.iter().any(|own| own.id == l.id));
    let own = def.links.iter().map(|l| (l, &def.names));
    inherited
        .chain(own)
        .filter_map(|(link, names)| {
            let text = |t: &Option<TextRef>| {
                t.as_ref()
                    .map(|t| t.resolve(names).text().trim().to_string())
                    .unwrap_or_default()
            };
            let (address, sub) = (text(&link.address), text(&link.sub_address));
            match (address.is_empty(), sub.is_empty()) {
                (true, true) => None,
                (false, true) => Some(address),
                (true, false) => Some(format!("#{sub}")),
                (false, false) => Some(format!("{address}#{sub}")),
            }
        })
        .collect()
}

/// A label without its keyboard accelerator: `Wall &Length` → `Wall
/// Length`, `R&&D` → `R&D`.
fn without_accelerator(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' if chars.peek() == Some(&'&') => {
                out.push('&');
                chars.next();
            }
            '&' => {}
            c => out.push(c),
        }
    }
    out
}

/// A numeric field as shown: in its unit, with the decimals its format
/// asks for; dates and times in ISO form.
pub(super) fn format_field(number: &Number, picture: Option<&str>) -> String {
    let Number {
        value,
        cell,
        power,
        mut format,
        ..
    } = *number;
    if !value.is_finite() {
        return String::new();
    }
    let mut custom = None;
    if let Some(picture) = picture {
        let builtin = picture
            .trim()
            .strip_prefix("esc(")
            .and_then(|rest| rest.strip_suffix(')'))
            .and_then(|code| code.trim().parse().ok());
        match builtin {
            Some(code) => format = Some(code),
            None => {
                if let Some((show_date, show_time)) = date_picture(picture) {
                    return date(value, show_date, show_time);
                }
                custom = Some(picture);
            }
        }
    }
    if cell == CELL_DATE || format.is_some_and(is_date_format) {
        let format = format.unwrap_or(0);
        let time = is_time_format(format);
        return date(value, !time, time || matches!(format, 0xD3 | 0xD4));
    }
    let (factor, unit) = unit(cell);
    // Feet and inches by format, or by unit with the decimals the format
    // asks for (`None`: as many as needed).
    let feet_inches = match format {
        Some(0x0A) => Some(Some(0)),
        Some(0x0D) => Some(Some(1)),
        Some(0x0E) => Some(Some(2)),
        Some(f @ 2..=9) if cell == CELL_FEET_INCHES => Some(Some(usize::from(f / 2 - 1))),
        None | Some(0 | 1) if cell == CELL_FEET_INCHES => Some(None),
        _ => None,
    };
    if let Some(decimals) = feet_inches
        && power == 1
        && factor != 0.0
        && (unit == "in" || unit == "ft" || unit == "mi" || unit.ends_with('m') || unit == "yd")
    {
        return feet_and_inches(value, decimals);
    }
    if cell == CELL_DEGREES_MINUTES_SECONDS && !matches!(format, Some(0x0B | 0x0C)) {
        return degrees_minutes_seconds(value);
    }
    let n = match unit {
        // Areas in square inches.
        "acre" | "ha" => value * factor,
        _ => value * factor.powi(i32::from(power)),
    };
    let label = match power {
        2 if unit != "acre" && unit != "ha" => format!("{unit}²"),
        3 => format!("{unit}³"),
        _ => unit.to_string(),
    };
    if let Some(shown) = custom.and_then(|p| number_picture(n, p, &label)) {
        return shown;
    }
    let (decimals, units) = match format {
        Some(0) => (None, false),
        Some(f @ 2..=9) => (Some(usize::from(f / 2 - 1)), f % 2 == 1),
        Some(0x0B) => return format!("{} rad", format_number(value)),
        Some(0x0C) => return format!("{} deg", format_number(value.to_degrees())),
        Some(0x0F | 0x11) => (None, false),
        _ => (None, true),
    };
    let number = match decimals {
        Some(d) => format!("{n:.d$}"),
        None => format_number(n),
    };
    match label.as_str() {
        "%" if units => format!("{number}%"),
        "" => number,
        unit if units => format!("{number} {unit}"),
        _ => number,
    }
}

/// A number shown by a custom picture such as `#,##0.00 u`: the digits as
/// its placeholders ask (`0` always shown, `#` when not zero), `u` (or
/// `uu`, `uuu`) as the unit, or as the currency symbol of a leading
/// `<locale,symbol>`; other text, quoted or not, as it is.
fn number_picture(value: f64, picture: &str, unit: &str) -> Option<String> {
    let picture = picture.trim();
    let picture = picture
        .strip_prefix("{{")
        .and_then(|p| p.strip_suffix("}}"))
        .unwrap_or(picture);
    let (currency, picture) = match picture.strip_prefix('<').and_then(|p| p.split_once('>')) {
        Some((spec, rest)) => (spec.rsplit(',').next().unwrap_or_default(), rest),
        None => ("", picture),
    };
    let start = picture.find(['#', '0'])?;
    let end = picture[start..]
        .find(|c| !matches!(c, '#' | '0' | ',' | '.'))
        .map_or(picture.len(), |n| start + n);
    let digits = &picture[start..end];
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let places = fraction.matches(['0', '#']).count();
    let mut optional = places - fraction.matches('0').count();
    let mut number = format!("{:.places$}", value.abs());
    while optional > 0 && number.ends_with('0') {
        number.pop();
        optional -= 1;
    }
    if number.ends_with('.') {
        number.pop();
    }
    if whole.contains(',') {
        let (int, rest) = number.split_at(number.find('.').unwrap_or(number.len()));
        number = format!("{}{rest}", group_thousands(int));
    }
    let negative = value < 0.0 && number.bytes().any(|b| matches!(b, b'1'..=b'9'));
    let label = if unit.is_empty() { currency } else { unit };
    let text = |s: &str| {
        let mut out = String::new();
        let (mut quoted, mut escaped, mut in_label) = (false, false, false);
        for c in s.chars() {
            let label_char = matches!(c, 'u' | 'U') && !quoted && !escaped;
            match c {
                _ if escaped => out.push(c),
                '\\' => {}
                '"' => quoted = !quoted,
                _ if label_char => {
                    if !in_label {
                        out.push_str(label);
                    }
                }
                _ => out.push(c),
            }
            in_label = label_char;
            escaped = c == '\\' && !escaped && !quoted;
        }
        out
    };
    let sign = if negative { "-" } else { "" };
    let shown = format!(
        "{}{sign}{number}{}",
        text(&picture[..start]),
        text(&picture[end..])
    );
    Some(shown.trim().to_string())
}

/// `1234567` → `1,234,567`.
fn group_thousands(digits: &str) -> String {
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A length in inches as feet and inches (`20'-6"`).
fn feet_and_inches(inches: f64, decimals: Option<usize>) -> String {
    let sign = if inches < 0.0 { "-" } else { "" };
    let scale = 10f64.powi(decimals.unwrap_or(6) as i32);
    let total = (inches.abs() * scale).round() / scale;
    let feet = (total / 12.0).floor();
    let rest = total - feet * 12.0;
    let rest = match decimals {
        Some(d) => format!("{rest:.d$}"),
        None => format_number(rest),
    };
    format!("{sign}{feet}'-{rest}\"")
}

/// An angle in radians as degrees, minutes and seconds (`130°49'9"`).
fn degrees_minutes_seconds(radians: f64) -> String {
    let sign = if radians < 0.0 { "-" } else { "" };
    let total = (radians.abs().to_degrees() * 3_600.0).round();
    let degrees = (total / 3_600.0).floor();
    let minutes = ((total - degrees * 3_600.0) / 60.0).floor();
    let seconds = total - degrees * 3_600.0 - minutes * 60.0;
    format!("{sign}{degrees}°{minutes}'{seconds}\"")
}

/// Whether a custom picture (`{{dd.MM.yyyy}}`) shows a date and a time.
fn date_picture(picture: &str) -> Option<(bool, bool)> {
    let inner: String = picture
        .split("{{")
        .skip(1)
        .filter_map(|part| part.split("}}").next())
        .collect();
    let date = inner.contains(['d', 'M', 'y', 'D', 'Y', 'g', 'e']);
    let time = inner.contains(['h', 'H', 's', 't', 'm']);
    (date || time).then_some((date, time))
}

/// How to show a value stored in a cell type (lengths are stored in
/// inches, angles in radians, percentages as fractions, durations in days).
fn unit(cell: u8) -> (f64, &'static str) {
    match cell {
        0x21 => (100.0, "%"),
        0x24 => (1.0 / 6_272_640.0, "acre"),
        0x25 => (1.0 / 15_500_031.0, "ha"),
        0x2B => (1.0 / 7.0, "ew."),
        0x2C => (1.0, "ed."),
        0x2D => (24.0, "eh."),
        0x2E => (1_440.0, "em."),
        0x2F => (86_400.0, "es."),
        0x31 | 0x32 => (72.0, "pt"),
        0x33 => (6.0, "p"),
        0x34 | 0x35 => (1.0 / 0.014_763_779_527_559, "d"),
        0x36 => (1.0 / 0.177_165_354_330_709, "c"),
        0x3F..=0x41 | 0x43 | 0x49 => (1.0, "in"),
        0x42 => (1.0 / 12.0, "ft"),
        0x44 | 0x4A => (1.0 / 63_360.0, "mi"),
        0x45 => (2.54, "cm"),
        0x46 => (25.4, "mm"),
        0x47 => (0.0254, "m"),
        0x48 => (0.000_025_4, "km"),
        0x4B => (1.0 / 36.0, "yd"),
        0x4C => (1.0 / 72_913.4, "nmi"),
        0x50 | 0x51 => (180.0 / std::f64::consts::PI, "deg"),
        0x53 => (1.0, "rad"),
        0x54 => (10_800.0 / std::f64::consts::PI, "'"),
        0x55 => (648_000.0 / std::f64::consts::PI, "\""),
        _ => (1.0, ""),
    }
}

fn is_date_format(format: u16) -> bool {
    matches!(format, 0x14..=0x24 | 0x2C..=0x51 | 0xC8..=0xDD)
}

/// Time-only display formats.
fn is_time_format(format: u16) -> bool {
    matches!(
        format,
        0x1E..=0x24 | 0x2E | 0x42..=0x4B | 0x50 | 0x51 | 0xD5..=0xD8
    )
}

/// A Visio date (days since 1899-12-30, the time as the fraction); the
/// time is shown when asked for or when the date has one.
fn date(value: f64, show_date: bool, show_time: bool) -> String {
    let days = value.floor();
    if !(-100_000.0..3_000_000.0).contains(&days) {
        return format_number(value);
    }
    let minutes = ((value - days) * 1_440.0).round() as i64;
    let (days, minutes) = (days as i64 + minutes / 1_440, minutes % 1_440);
    let time = format!("{:02}:{:02}", minutes / 60, minutes % 60);
    if !show_date {
        return time;
    }
    let (y, m, d) = civil_from_days(days - DAYS_TO_UNIX_EPOCH);
    let date = format!("{y:04}-{m:02}-{d:02}");
    if show_time || minutes != 0 {
        format!("{date} {time}")
    } else {
        date
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(value: f64, cell: u8, format: Option<u16>) -> Number {
        Number {
            value,
            cell,
            power: 1,
            format,
            picture: None,
        }
    }

    #[test]
    fn numeric_fields_show_their_units() {
        // 1 cm, one decimal with units (format 5); without units (4).
        let cm = 1.0 / 2.54;
        assert_eq!(format_field(&number(cm, 0x45, Some(5)), None), "1.0 cm");
        assert_eq!(format_field(&number(1.0, 0x20, Some(4)), None), "1.0");
        assert_eq!(format_field(&number(1.0, 0x20, Some(0x0B)), None), "1 rad");
        assert_eq!(format_field(&number(0.01, 0x21, Some(1)), None), "1%");
        assert_eq!(format_field(&number(1.0 / 72.0, 0x32, None), None), "1 pt");
        assert_eq!(format_field(&number(1.0, 0x20, None), Some("esc(3)")), "1");
        assert_eq!(
            format_field(&number(246.0, 0x40, Some(0x0A)), None),
            "20'-6\""
        );
        assert_eq!(
            format_field(&number(12_001.2, 0x43, Some(9)), None),
            "1000'-1.200\""
        );
        assert_eq!(format_field(&number(246.5, 0x43, None), None), "20'-6.5\"");
        let angle = (130.0 + 49.0 / 60.0 + 9.0 / 3_600.0f64).to_radians();
        assert_eq!(
            format_field(&number(angle, 0x52, Some(1)), None),
            "130°49'9\""
        );
    }

    /// A formula block holding a string literal for `cell`.
    fn string_block(cell: u8, text: &str) -> Vec<u8> {
        let chars: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut block = vec![
            0,
            0,
            0,
            0,
            2,
            cell,
            TOKEN_STRING,
            text.encode_utf16().count() as u8,
        ];
        block.extend(chars);
        block.extend([0, 0, 0xFE]);
        let length = block.len() as u32;
        block[..4].copy_from_slice(&length.to_le_bytes());
        block
    }

    #[test]
    fn shape_data_rows_take_strings_from_formulas() {
        // Value: a string in the value formula; label in the label
        // formula; no prompt, format or sort key; type string.
        let mut row = vec![CELL_STRING];
        row.extend((-3i32).to_le_bytes());
        row.extend([0; 4]);
        for id in [-2i32, -3, -2, -2] {
            row.extend(id.to_le_bytes());
        }
        row.push(PROP_STRING);
        row.resize(36, 0);
        row.extend(string_block(PROP_VALUE, "Alice"));
        row.extend(string_block(PROP_LABEL, "&Owner"));
        row.extend([0; 4]);
        let strings = formula_strings(Version::V11, &row, PROP_FIXED);
        assert_eq!(strings.get(&PROP_VALUE).map(String::as_str), Some("Alice"));
        assert_eq!(strings.get(&PROP_LABEL).map(String::as_str), Some("&Owner"));
        assert_eq!(without_accelerator("R&&D &Budget"), "R&D Budget");
    }

    #[test]
    fn pictures_place_digits_and_symbols() {
        let n = |value| number(value, 0x20, Some(1));
        assert_eq!(format_field(&n(1.0), Some("<,$>U #,##0.00")), "$ 1.00");
        assert_eq!(format_field(&n(-1234.5), Some("#,##0.00 U")), "-1,234.50");
        assert_eq!(format_field(&n(2.5), Some("{{0.0#}}")), "2.5");
        assert_eq!(format_field(&n(2.0), Some("0 \"units\"")), "2 units");
        let cm = number(1.0 / 2.54, 0x45, Some(1));
        assert_eq!(format_field(&cm, Some("0.00 u")), "1.00 cm");
        let area = Number {
            power: 2,
            ..number(1.0 / (2.54 * 2.54), 0x45, Some(1))
        };
        assert_eq!(format_field(&area, None), "1 cm²");
    }

    #[test]
    fn dates_are_iso() {
        assert_eq!(
            format_field(&number(43_622.0, 0x28, None), None),
            "2019-06-06"
        );
        assert_eq!(
            format_field(&number(43_622.5, 0x28, Some(0xC8)), None),
            "2019-06-06 12:00"
        );
        assert_eq!(format_field(&number(0.75, 0x28, Some(0x1F)), None), "18:00");
        assert_eq!(
            format_field(
                &number(37_773.0, 0x20, None),
                Some("{{dddd, d. MMMM yyyy}}")
            ),
            "2003-06-01"
        );
    }

    #[test]
    fn formulas_name_formats_and_areas() {
        assert_eq!(
            format_token(&[0, 2, 2, 0x62, 0x12, 0, 0x80, 0xC2]),
            Some(0x12)
        );
        let mut formula = vec![0x46];
        formula.extend_from_slice(&6_272_640.0f64.to_le_bytes());
        formula.extend_from_slice(&[0x24, 2]);
        assert_eq!(dimensioned(&formula), Some((6_272_640.0, 0x24, 2)));
    }

    #[test]
    fn text_is_split_at_fields() {
        let pieces = text(Version::V6, b"\0\0\0\0\0\0\0\0Width \x1e mm\0junk");
        let fields = [Field::Number(number(1.0, 0x41, Some(3)))];
        assert_eq!(render_text(&pieces, &fields, &|_| None), "Width 1 in mm");
        // Version 5 keeps three bytes of field information after the mark.
        let pieces = text(Version::V5, b"\0\0\0\0\0\0\0\0Angle \x1e  %\0");
        assert_eq!(render_text(&pieces, &[], &|_| None), "Angle");
    }

    #[test]
    fn glue_blocks_name_the_glued_shapes() {
        let mut data = vec![0u8; 23];
        let mut block = vec![0u8; 20];
        block[0..4].copy_from_slice(&20u32.to_le_bytes());
        block[4] = 2;
        block[6] = 0x74;
        block[7..11].copy_from_slice(&0x6000_004Eu32.to_le_bytes());
        block[11..15].copy_from_slice(&7u32.to_le_bytes());
        block[15] = 0x7A;
        block[16..20].copy_from_slice(&0x4000_0073u32.to_le_bytes());
        data.extend_from_slice(&block);
        assert_eq!(glue(Version::V6, &data), [7]);
    }
}
