//! A small, namespace-normalising XML tree for Office Open XML parts.
//!
//! Office files bind the same namespaces to different prefixes (and Strict
//! OOXML uses different namespace URIs altogether), so element and attribute
//! names are rewritten to one canonical prefix per namespace: `w:p` is a
//! WordprocessingML paragraph no matter how the file spells it. Names in
//! namespaces we do not know keep the prefix used in the file.
//!
//! Markup-compatibility blocks (`mc:AlternateContent`) are resolved while
//! parsing: the first `mc:Choice` whose required namespaces we know replaces
//! the block, otherwise the `mc:Fallback` does.

use std::borrow::Cow;

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{QName, ResolveResult};
use quick_xml::reader::NsReader;

/// Namespace URIs and the canonical prefix each one is given.
const NAMESPACES: &[(&str, &str)] = &[
    // Package level.
    (
        "http://schemas.openxmlformats.org/package/2006/content-types",
        "ct",
    ),
    (
        "http://schemas.openxmlformats.org/package/2006/relationships",
        "rel",
    ),
    (
        "http://schemas.openxmlformats.org/package/2006/metadata/core-properties",
        "cp",
    ),
    ("http://purl.org/dc/elements/1.1/", "dc"),
    ("http://purl.org/dc/terms/", "dcterms"),
    ("http://www.w3.org/XML/1998/namespace", "xml"),
    ("http://www.w3.org/2001/XMLSchema-instance", "xsi"),
    (
        "http://schemas.openxmlformats.org/markup-compatibility/2006",
        "mc",
    ),
    // Shared Office Open XML namespaces (transitional, then strict).
    (
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
        "r",
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/relationships",
        "r",
    ),
    (
        "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties",
        "ep",
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/extendedProperties",
        "ep",
    ),
    (
        "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties",
        "op",
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/customProperties",
        "op",
    ),
    (
        "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes",
        "vt",
    ),
    (
        "http://purl.oclc.org/ooxml/officeDocument/docPropsVTypes",
        "vt",
    ),
    (
        "http://schemas.openxmlformats.org/officeDocument/2006/math",
        "m",
    ),
    ("http://purl.oclc.org/ooxml/officeDocument/math", "m"),
    // WordprocessingML.
    (
        "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
        "w",
    ),
    ("http://purl.oclc.org/ooxml/wordprocessingml/main", "w"),
    (
        "http://schemas.microsoft.com/office/word/2010/wordml",
        "w14",
    ),
    (
        "http://schemas.microsoft.com/office/word/2012/wordml",
        "w15",
    ),
    (
        "http://schemas.microsoft.com/office/word/2010/wordprocessingShape",
        "wps",
    ),
    (
        "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup",
        "wpg",
    ),
    (
        "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas",
        "wpc",
    ),
    (
        "http://schemas.microsoft.com/office/word/2010/wordprocessingInk",
        "wpi",
    ),
    // SpreadsheetML.
    (
        "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
        "x",
    ),
    ("http://purl.oclc.org/ooxml/spreadsheetml/main", "x"),
    (
        "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main",
        "x14",
    ),
    (
        "http://schemas.microsoft.com/office/spreadsheetml/2018/threadedcomments",
        "tc",
    ),
    // PresentationML.
    (
        "http://schemas.openxmlformats.org/presentationml/2006/main",
        "p",
    ),
    ("http://purl.oclc.org/ooxml/presentationml/main", "p"),
    (
        "http://schemas.microsoft.com/office/powerpoint/2010/main",
        "p14",
    ),
    (
        "http://schemas.microsoft.com/office/powerpoint/2018/8/main",
        "p188",
    ),
    // DrawingML.
    ("http://schemas.openxmlformats.org/drawingml/2006/main", "a"),
    ("http://purl.oclc.org/ooxml/drawingml/main", "a"),
    (
        "http://schemas.microsoft.com/office/drawing/2010/main",
        "a14",
    ),
    (
        "http://schemas.openxmlformats.org/drawingml/2006/picture",
        "pic",
    ),
    ("http://purl.oclc.org/ooxml/drawingml/picture", "pic"),
    (
        "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing",
        "wp",
    ),
    (
        "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing",
        "wp",
    ),
    (
        "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing",
        "xdr",
    ),
    (
        "http://purl.oclc.org/ooxml/drawingml/spreadsheetDrawing",
        "xdr",
    ),
    (
        "http://schemas.openxmlformats.org/drawingml/2006/chart",
        "c",
    ),
    ("http://purl.oclc.org/ooxml/drawingml/chart", "c"),
    (
        "http://schemas.microsoft.com/office/drawing/2014/chartex",
        "cx",
    ),
    (
        "http://schemas.openxmlformats.org/drawingml/2006/diagram",
        "dgm",
    ),
    ("http://purl.oclc.org/ooxml/drawingml/diagram", "dgm"),
    (
        "http://schemas.microsoft.com/office/drawing/2008/diagram",
        "dsp",
    ),
    // Legacy VML.
    ("urn:schemas-microsoft-com:vml", "v"),
    ("urn:schemas-microsoft-com:office:office", "o"),
    ("urn:schemas-microsoft-com:office:excel", "xv"),
    ("urn:schemas-microsoft-com:office:word", "wv"),
    // Visio.
    ("http://schemas.microsoft.com/office/visio/2012/main", "vis"),
];

/// Namespaces whose content readers can use when choosing between the
/// branches of an `mc:AlternateContent` block. Newer DrawingML variants
/// (`wps`, `a14`) are preferred over their VML fallbacks.
fn understood(uri: &str) -> bool {
    NAMESPACES.iter().any(|(u, _)| *u == uri)
}

fn canonical_prefix(uri: &str) -> Option<&'static str> {
    NAMESPACES.iter().find(|(u, _)| *u == uri).map(|(_, p)| *p)
}

/// Elements deeper than this are dropped, so hostile input cannot exhaust the
/// stack of readers that walk the tree recursively.
const MAX_DEPTH: usize = 256;

/// One element.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    nodes: Vec<Node>,
}

/// Content of an element.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    Element(Element),
    Text(String),
}

impl Element {
    /// Canonical qualified name, such as `w:p`.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Name without its prefix.
    pub(crate) fn local(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    /// Whether the element has the given canonical name.
    pub(crate) fn is(&self, name: &str) -> bool {
        self.name == name
    }

    /// An attribute value by canonical name (`r:id`, or `val` for
    /// attributes without a namespace).
    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// An attribute parsed as a number.
    pub(crate) fn attr_parse<T: std::str::FromStr>(&self, name: &str) -> Option<T> {
        self.attr(name)?.trim().parse().ok()
    }

    /// An `xsd:boolean`/`ST_OnOff` attribute: `1`, `true`, `on` → true.
    pub(crate) fn attr_bool(&self, name: &str) -> Option<bool> {
        match self.attr(name)?.trim() {
            "1" | "true" | "on" | "t" => Some(true),
            "0" | "false" | "off" | "f" => Some(false),
            _ => None,
        }
    }

    /// Child nodes (elements and text) in document order.
    pub(crate) fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Child elements in document order.
    pub(crate) fn children(&self) -> impl Iterator<Item = &Element> {
        self.nodes.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    /// The first child element with the given name.
    pub(crate) fn child(&self, name: &str) -> Option<&Element> {
        self.children().find(|e| e.is(name))
    }

    /// The first child with the given local name, whatever its namespace
    /// (`p:txBody`, `xdr:txBody`).
    pub(crate) fn child_local(&self, local: &str) -> Option<&Element> {
        self.children().find(|e| e.local() == local)
    }

    /// All child elements with the given name.
    pub(crate) fn children_named<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a Element> + 'a {
        self.children().filter(move |e| e.is(name))
    }

    /// Follow a `/`-separated path of child names (`p:cSld/p:spTree`).
    pub(crate) fn path(&self, path: &str) -> Option<&Element> {
        path.split('/').try_fold(self, |el, name| el.child(name))
    }

    /// The value of the `val` (or `w:val`) attribute of a child, a common
    /// OOXML idiom: `<w:pStyle w:val="Heading1"/>`.
    pub(crate) fn child_val(&self, name: &str) -> Option<&str> {
        let child = self.child(name)?;
        child.attr("w:val").or_else(|| child.attr("val"))
    }

    /// Every descendant element, depth first, in document order.
    pub(crate) fn descendants(&self) -> Descendants<'_> {
        Descendants {
            stack: vec![self.children_slice()],
        }
    }

    /// The first descendant with the given name.
    pub(crate) fn find(&self, name: &str) -> Option<&Element> {
        self.descendants().find(|e| e.is(name))
    }

    /// All descendants with the given name.
    pub(crate) fn find_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.descendants().filter(move |e| e.is(name))
    }

    /// All text below this element, concatenated.
    pub(crate) fn text(&self) -> String {
        let mut out = String::new();
        self.collect_text(&mut out);
        out
    }

    fn collect_text(&self, out: &mut String) {
        for node in &self.nodes {
            match node {
                Node::Text(t) => out.push_str(t),
                Node::Element(e) => e.collect_text(out),
            }
        }
    }

    fn children_slice(&self) -> std::slice::Iter<'_, Node> {
        self.nodes.iter()
    }
}

/// Iterator over the descendants of an element.
#[derive(Debug)]
pub(crate) struct Descendants<'a> {
    stack: Vec<std::slice::Iter<'a, Node>>,
}

impl<'a> Iterator for Descendants<'a> {
    type Item = &'a Element;

    fn next(&mut self) -> Option<&'a Element> {
        loop {
            let iter = self.stack.last_mut()?;
            match iter.next() {
                Some(Node::Element(e)) => {
                    self.stack.push(e.children_slice());
                    return Some(e);
                }
                Some(Node::Text(_)) => {}
                None => {
                    self.stack.pop();
                }
            }
        }
    }
}

/// The result of parsing: the tree plus, for damaged input, the reason
/// parsing stopped early. Everything read up to that point is kept.
#[derive(Debug)]
pub(crate) struct Parsed {
    pub(crate) root: Element,
    pub(crate) error: Option<String>,
}

/// Parse an XML part.
///
/// Whitespace-only text containing a line break is dropped (it is almost
/// always indentation), except in elements listed in `mixed`, whose text
/// is significant everywhere (Visio's `vis:Text`).
pub(crate) fn parse(data: &[u8]) -> Parsed {
    parse_with(data, &[])
}

/// [`parse`], keeping all whitespace inside the `mixed` elements.
pub(crate) fn parse_with(data: &[u8], mixed: &[&str]) -> Parsed {
    let text = decode(data);
    let mut reader = NsReader::from_str(&text);
    let config = reader.config_mut();
    config.trim_text(false);
    config.expand_empty_elements = false;
    config.check_end_names = false;

    let mut builder = Builder {
        stack: vec![Element::default()],
        skipped: 0,
        mixed,
    };
    let error = loop {
        let event = match reader.read_resolved_event() {
            Ok((_, event)) => event,
            Err(e) => break Some(format!("{e} at byte {}", reader.error_position())),
        };
        match event {
            Event::Start(start) => {
                let el = start_element(&reader, &start);
                builder.open(el);
            }
            Event::Empty(start) => {
                let el = start_element(&reader, &start);
                builder.open(el);
                builder.close();
            }
            Event::End(_) => builder.close(),
            Event::Text(t) => builder.text(&t.xml10_content()),
            Event::CData(t) => builder.text(&t.into_inner()),
            Event::GeneralRef(r) => builder.text(&resolve_reference(&r)),
            Event::Eof => break None,
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    };
    // Close whatever a truncated document left open.
    while builder.stack.len() > 1 {
        builder.close();
    }
    let document = builder.stack.pop().unwrap_or_default();
    let root = document.nodes.into_iter().find_map(|n| match n {
        Node::Element(e) => Some(e),
        Node::Text(_) => None,
    });
    match root {
        Some(root) => Parsed { root, error },
        None => Parsed {
            root: Element::default(),
            error: Some(error.unwrap_or_else(|| "no root element".into())),
        },
    }
}

/// Collect only the elements named in `keep` (with everything inside them)
/// from a part that may be too large to hold as a tree, such as a
/// worksheet with a million cells. Markup-compatibility blocks around the
/// kept elements are not resolved: an element present in both branches is
/// returned twice.
pub(crate) fn parse_filtered(data: &[u8], keep: &[&str]) -> Vec<Element> {
    let text = decode(data);
    let mut reader = NsReader::from_str(&text);
    let config = reader.config_mut();
    config.trim_text(false);
    config.check_end_names = false;
    let locals: Vec<&str> = keep
        .iter()
        .map(|k| k.rsplit(':').next().unwrap_or(k))
        .collect();
    let mut builder = Builder {
        stack: vec![Element::default()],
        skipped: 0,
        mixed: &[],
    };
    while let Ok((_, event)) = reader.read_resolved_event() {
        let inside = builder.stack.len() > 1 || builder.skipped > 0;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(start) | Event::Empty(start) => {
                let wanted = inside || {
                    let local = start.local_name().into_inner();
                    locals.contains(&local)
                        && keep.contains(
                            &canonical_name(
                                reader.resolver().resolve_element(start.name()),
                                start.name(),
                            )
                            .as_str(),
                        )
                };
                if wanted {
                    builder.open(start_element(&reader, &start));
                    if empty {
                        builder.close();
                    }
                }
            }
            Event::End(_) if inside => builder.close(),
            Event::Text(t) if inside => builder.text(&t.xml10_content()),
            Event::GeneralRef(r) if inside => builder.text(&resolve_reference(&r)),
            Event::Eof => break,
            _ => {}
        }
    }
    while builder.stack.len() > 1 {
        builder.close();
    }
    builder
        .stack
        .pop()
        .map(|root| {
            root.nodes
                .into_iter()
                .filter_map(|n| match n {
                    Node::Element(e) => Some(e),
                    Node::Text(_) => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn resolve_reference(r: &quick_xml::events::BytesRef<'_>) -> String {
    match r.resolve_char_ref() {
        Ok(Some(c)) => c.to_string(),
        _ => quick_xml::escape::resolve_predefined_entity(&r.borrow().into_inner())
            .map(str::to_string)
            .unwrap_or_default(),
    }
}

/// Turn the raw bytes into text: UTF-16 is transcoded, a UTF-8 byte-order
/// mark is dropped, invalid UTF-8 is replaced rather than rejected.
fn decode(data: &[u8]) -> Cow<'_, str> {
    if let Some((encoding, bom)) = encoding_rs::Encoding::for_bom(data) {
        let (text, _) = encoding.decode_without_bom_handling(&data[bom..]);
        return text;
    }
    // UTF-16 without a byte-order mark: `<` followed by a zero byte.
    if data.starts_with(b"<\0") {
        return encoding_rs::UTF_16LE.decode_without_bom_handling(data).0;
    }
    if data.starts_with(b"\0<") {
        return encoding_rs::UTF_16BE.decode_without_bom_handling(data).0;
    }
    String::from_utf8_lossy(data)
}

fn start_element(reader: &NsReader<&[u8]>, start: &BytesStart<'_>) -> Element {
    let resolver = reader.resolver();
    let name = canonical_name(resolver.resolve_element(start.name()), start.name());
    let mut attrs = Vec::new();
    for attr in start.attributes().with_checks(false).flatten() {
        let key = attr.key.0;
        if key == "xmlns" || key.starts_with("xmlns:") {
            continue;
        }
        let key_name = canonical_name(resolver.resolve_attribute(attr.key), attr.key);
        let value = match attr.normalized_value(XmlVersion::Implicit1_0) {
            Ok(v) => v.into_owned(),
            Err(_) => attr.value.into_owned(),
        };
        attrs.push((key_name, value));
    }
    let mut el = Element {
        name,
        attrs,
        nodes: Vec::new(),
    };
    if el.is("mc:Choice") {
        // Resolve the prefixes listed in `Requires` now, while their
        // declarations are in scope.
        let usable = el.attr("Requires").is_none_or(|req| {
            req.split_whitespace().all(|prefix| {
                let probe = format!("{prefix}:x");
                match resolver.resolve_element(QName(&probe)).0 {
                    ResolveResult::Bound(ns) => understood(ns.0),
                    _ => false,
                }
            })
        });
        el.attrs.push((USABLE.into(), usable.to_string()));
    }
    el
}

/// Internal marker attribute on `mc:Choice` elements.
const USABLE: &str = "\u{0}usable";

fn canonical_name(
    resolved: (ResolveResult<'_>, quick_xml::name::LocalName<'_>),
    raw: QName<'_>,
) -> String {
    let (ns, local) = resolved;
    let local = local.into_inner();
    match ns {
        ResolveResult::Bound(ns) => match canonical_prefix(ns.0) {
            Some(prefix) => format!("{prefix}:{local}"),
            None => raw.0.to_string(),
        },
        _ => raw.0.to_string(),
    }
}

struct Builder<'m> {
    stack: Vec<Element>,
    /// Depth of elements being skipped because they are nested too deeply.
    skipped: usize,
    mixed: &'m [&'m str],
}

impl Builder<'_> {
    fn open(&mut self, el: Element) {
        if self.skipped > 0 || self.stack.len() > MAX_DEPTH {
            self.skipped += 1;
            return;
        }
        self.stack.push(el);
    }

    fn close(&mut self) {
        if self.skipped > 0 {
            self.skipped -= 1;
            return;
        }
        if self.stack.len() <= 1 {
            return; // stray end tag
        }
        let Some(mut el) = self.stack.pop() else {
            return;
        };
        if !self.mixed.contains(&el.name.as_str()) && el.children().next().is_some() {
            el.nodes.retain(|n| match n {
                Node::Text(t) => !(t.trim().is_empty() && t.contains('\n')),
                Node::Element(_) => true,
            });
        }
        let Some(parent) = self.stack.last_mut() else {
            return;
        };
        if el.is("mc:AlternateContent") {
            let chosen = el
                .children()
                .position(|c| c.is("mc:Choice") && c.attr(USABLE) == Some("true"))
                .or_else(|| el.children().position(|c| c.is("mc:Fallback")));
            if let Some(index) = chosen
                && let Some(branch) = el
                    .nodes
                    .into_iter()
                    .filter_map(|n| match n {
                        Node::Element(e) => Some(e),
                        Node::Text(_) => None,
                    })
                    .nth(index)
            {
                parent.nodes.extend(branch.nodes);
            }
            return;
        }
        parent.nodes.push(Node::Element(el));
    }

    fn text(&mut self, text: &str) {
        if self.skipped > 0 || text.is_empty() {
            return;
        }
        let Some(el) = self.stack.last_mut() else {
            return;
        };
        if let Some(Node::Text(last)) = el.nodes.last_mut() {
            last.push_str(text);
        } else {
            el.nodes.push(Node::Text(text.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_canonicalised_across_prefixes_and_strict_namespaces() {
        let xml = br#"<?xml version="1.0"?>
            <doc:document xmlns:doc="http://purl.oclc.org/ooxml/wordprocessingml/main"
                          xmlns:rr="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
              <doc:body><doc:p doc:rsidR="00A1" rr:id="rId4"><doc:r><doc:t xml:space="preserve"> a &amp; b &#x263A; </doc:t></doc:r></doc:p></doc:body>
            </doc:document>"#;
        let parsed = parse(xml);
        assert!(parsed.error.is_none());
        let root = parsed.root;
        assert!(root.is("w:document"));
        let p = root.path("w:body/w:p").unwrap();
        assert_eq!(p.attr("r:id"), Some("rId4"));
        assert_eq!(p.attr("w:rsidR"), Some("00A1"));
        let t = p.find("w:t").unwrap();
        assert_eq!(t.text(), " a & b \u{263A} ");
        assert_eq!(t.attr("xml:space"), Some("preserve"));
    }

    #[test]
    fn indentation_is_dropped_but_significant_spaces_are_kept() {
        let xml = b"<Text xmlns='http://schemas.microsoft.com/office/visio/2012/main'>\n  <cp IX='0'/>A<cp IX='1'/>\n<cp IX='2'/>B</Text>";
        let mixed = parse_with(xml, &["vis:Text"]).root;
        assert_eq!(mixed.text(), "\n  A\nB");
        let plain = parse(xml).root;
        assert_eq!(plain.text(), "AB");
    }

    #[test]
    fn alternate_content_picks_a_known_choice_or_the_fallback() {
        let xml = br#"<root xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
                            xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
                            xmlns:zz="urn:unknown">
            <mc:AlternateContent><mc:Choice Requires="wps"><wps:wsp/></mc:Choice><mc:Fallback><fallback/></mc:Fallback></mc:AlternateContent>
            <mc:AlternateContent><mc:Choice Requires="zz"><zz:new/></mc:Choice><mc:Fallback><old/></mc:Fallback></mc:AlternateContent>
        </root>"#;
        let root = parse(xml).root;
        let names: Vec<&str> = root.children().map(Element::name).collect();
        assert_eq!(names, ["wps:wsp", "old"]);
    }

    #[test]
    fn damaged_input_keeps_what_was_read() {
        let parsed = parse(b"<a><b>one</b><c>two");
        assert_eq!(parsed.root.text(), "onetwo");
        let parsed = parse(b"<a><b>one</b></x></a><<<");
        assert!(parsed.root.is("a"));
        assert!(parsed.root.text().starts_with("one"));
        let parsed = parse(b"");
        assert!(parsed.error.is_some());
    }

    #[test]
    fn utf16_input_is_decoded() {
        let xml = "\u{feff}<a>h\u{e9}</a>";
        let bytes: Vec<u8> = xml.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(parse(&bytes).root.text(), "h\u{e9}");
    }

    #[test]
    fn filtered_parsing_keeps_only_wanted_subtrees() {
        let xml = br#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
            <sheetData><row r="1"><c r="A1"><v>1</v></c></row></sheetData>
            <oleObjects><oleObject progId="Word.Document.12" r:id="rId3"><objectPr><anchor/></objectPr></oleObject></oleObjects>
            <hyperlinks><hyperlink ref="A1" r:id="rId1"/></hyperlinks>
        </worksheet>"#;
        let found = parse_filtered(xml, &["x:oleObject", "x:hyperlink"]);
        let names: Vec<&str> = found.iter().map(Element::name).collect();
        assert_eq!(names, ["x:oleObject", "x:hyperlink"]);
        assert_eq!(found[0].attr("r:id"), Some("rId3"));
        assert!(found[0].find("x:anchor").is_some());
    }

    #[test]
    fn deep_nesting_is_cut_off() {
        let xml = "<a>".repeat(1000) + "x" + &"</a>".repeat(1000);
        let root = parse(xml.as_bytes()).root;
        assert!(root.descendants().count() <= MAX_DEPTH);
    }
}
