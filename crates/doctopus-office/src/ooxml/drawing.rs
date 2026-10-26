//! DrawingML, the drawing language shared by all Office Open XML formats:
//! text bodies, tables, pictures, and the graphic frames that hold charts,
//! SmartArt and embedded objects.

use doctopus_core::{
    Block, EmbedRef, ImageRef, Inline, Link, ListItem, Math, Style, Table, TableCell, TableRow,
    push_text,
};

use super::chart;
use super::context::Context;
use super::math;
use super::package::Relationships;
use super::smartart;
use super::xml::Element;
use crate::blocks::{link_shape, push_text_box};
use crate::numbering::auto_number_scheme;

/// How paragraphs without explicit bullet formatting are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bullets {
    /// Only explicit `a:buChar`/`a:buAutoNum` make list items (text boxes).
    Explicit,
    /// Paragraphs are bulleted unless `a:buNone` says otherwise (body
    /// placeholders, which inherit bullets from the slide master).
    Inherited,
}

/// Read a text body (`a:txBody`, `p:txBody`, `xdr:txBody`, `c:rich`).
pub(crate) fn text_body(rels: &Relationships, body: &Element, bullets: Bullets) -> Vec<Block> {
    let mut out = Vec::new();
    let mut counters: [Option<(String, u32)>; 9] = Default::default();
    for p in body.children_named("a:p") {
        let content = paragraph_inlines(rels, p);
        if inlines_blank(&content) {
            continue;
        }
        let ppr = p.child("a:pPr");
        let level = ppr
            .and_then(|ppr| ppr.attr_parse::<usize>("lvl"))
            .unwrap_or(0)
            .min(8);
        let explicit_none = ppr.is_some_and(|ppr| ppr.child("a:buNone").is_some());
        let auto = ppr.and_then(|ppr| ppr.child("a:buAutoNum"));
        let bullet = ppr
            .is_some_and(|ppr| ppr.child("a:buChar").is_some() || ppr.child("a:buBlip").is_some());
        if let Some(auto) = auto.filter(|_| !explicit_none) {
            let scheme = auto.attr("type").unwrap_or("arabicPeriod").to_string();
            let start = auto.attr_parse::<u32>("startAt").unwrap_or(1);
            let n = match &counters[level] {
                Some((s, n)) if *s == scheme => n + 1,
                _ => start,
            };
            counters[level] = Some((scheme.clone(), n));
            for deeper in counters.iter_mut().skip(level + 1) {
                *deeper = None;
            }
            let (style, before, after) = auto_number_scheme(&scheme);
            out.push(Block::ListItem(ListItem {
                level: level as u8,
                label: Some(format!("{before}{}{after}", style.format(n))),
                content,
            }));
            continue;
        }
        for counter in counters.iter_mut().skip(level) {
            *counter = None;
        }
        let is_bullet = !explicit_none && (bullet || bullets == Bullets::Inherited);
        if is_bullet {
            out.push(Block::ListItem(ListItem {
                level: level as u8,
                label: None,
                content,
            }));
        } else {
            out.push(Block::paragraph(content));
        }
    }
    out
}

/// The inline content of one `a:p`.
pub(crate) fn paragraph_inlines(rels: &Relationships, p: &Element) -> Vec<Inline> {
    let mut content: Vec<Inline> = Vec::new();
    for el in p.children() {
        match el.name() {
            "a:r" | "a:fld" => {
                let text = el.child("a:t").map(|t| t.text()).unwrap_or_default();
                let text = text.replace('\u{b}', "\n");
                let rpr = el.child("a:rPr");
                let style = run_style(rpr);
                let link = rpr
                    .and_then(|r| r.child("a:hlinkClick"))
                    .and_then(|h| h.attr("r:id"))
                    .and_then(|id| Context::hyperlink(rels, id));
                match link {
                    Some(url) => push_link(&mut content, url, &text, style),
                    None => push_text(&mut content, &text, style),
                }
            }
            "a:br" => content.push(Inline::LineBreak),
            "a14:m" | "m:oMathPara" | "m:oMath" => {
                let math = el
                    .descendants()
                    .find(|e| e.is("m:oMathPara") || e.is("m:oMath"))
                    .or(Some(el).filter(|e| e.name().starts_with("m:")));
                if let Some(math) = math {
                    push_math(&mut content, math::to_tex(math));
                }
            }
            _ => {}
        }
    }
    content
}

/// Append an equation unless it is empty.
pub(crate) fn push_math(content: &mut Vec<Inline>, tex: String) {
    if !tex.trim().is_empty() {
        content.push(Inline::Math(Math { tex }));
    }
}

/// Character formatting of an `a:rPr`.
fn run_style(rpr: Option<&Element>) -> Style {
    let Some(rpr) = rpr else {
        return Style::default();
    };
    let baseline = rpr.attr_parse::<i64>("baseline").unwrap_or(0);
    Style {
        bold: rpr.attr_bool("b").unwrap_or(false),
        italic: rpr.attr_bool("i").unwrap_or(false),
        strike: rpr.attr("strike").is_some_and(|s| s != "noStrike"),
        superscript: baseline > 0,
        subscript: baseline < 0,
    }
}

/// Append linked text, extending the previous link when it has the same
/// target (runs are split by formatting changes, links should not be).
pub(crate) fn push_link(content: &mut Vec<Inline>, url: String, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(Inline::Link(last)) = content.last_mut()
        && last.url == url
    {
        push_text(&mut last.content, text, style);
        return;
    }
    let mut inner = Vec::new();
    push_text(&mut inner, text, style);
    content.push(Inline::Link(Link {
        url,
        content: inner,
    }));
}

fn inlines_blank(content: &[Inline]) -> bool {
    content.iter().all(|i| match i {
        Inline::Text(t) => t.text.trim().is_empty(),
        Inline::LineBreak => true,
        Inline::Link(l) => inlines_blank(&l.content),
        Inline::NoteRef(_) | Inline::Image(_) | Inline::Math(_) => false,
    })
}

/// Read an `a:tbl`.
pub(crate) fn table(rels: &Relationships, tbl: &Element) -> Table {
    let header_rows = u32::from(
        tbl.child("a:tblPr")
            .and_then(|p| p.attr_bool("firstRow"))
            .unwrap_or(false),
    );
    let rows = tbl
        .children_named("a:tr")
        .map(|tr| TableRow {
            cells: tr
                .children_named("a:tc")
                .map(|tc| {
                    let blocks = tc
                        .child("a:txBody")
                        .map(|body| text_body(rels, body, Bullets::Explicit))
                        .unwrap_or_default();
                    let mut cell = TableCell::new(blocks);
                    cell.col_span = tc.attr_parse("gridSpan").unwrap_or(1).max(1);
                    cell.row_span = tc.attr_parse("rowSpan").unwrap_or(1).max(1);
                    cell.merged = tc.attr_bool("hMerge").unwrap_or(false)
                        || tc.attr_bool("vMerge").unwrap_or(false);
                    cell
                })
                .collect(),
        })
        .collect();
    Table { rows, header_rows }
}

/// The alternative text of a drawing object (`*:cNvPr` `descr`, else
/// `title`).
pub(crate) fn alt_text(el: &Element) -> Option<String> {
    let props = el
        .descendants()
        .find(|e| e.local() == "cNvPr" || e.is("wp:docPr"))?;
    props
        .attr("descr")
        .or_else(|| props.attr("title"))
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
}

/// Read a picture (`p:pic`, `pic:pic`, `xdr:pic`): an image, or an audio
/// or video clip with its poster frame.
pub(crate) fn picture(
    cx: &mut Context<'_>,
    rels: &Relationships,
    pic: &Element,
    location: &str,
) -> Option<Block> {
    // Audio and video on slides: the picture is only the poster frame.
    let media = pic.descendants().find_map(|e| match e.name() {
        "a:videoFile" | "a:audioFile" | "a:quickTimeFile" => e.attr("r:link"),
        "p14:media" => e.attr("r:embed").or_else(|| e.attr("r:link")),
        _ => None,
    });
    if let Some(id) = media
        && let Some(part) = cx.media(rels, id, location)
    {
        return Some(Block::Embed(EmbedRef { part }));
    }
    let blip = pic.find("a:blip")?;
    let id = blip.attr("r:embed").or_else(|| blip.attr("r:link"))?;
    let part = cx.image(rels, id, location)?;
    Some(Block::Image(ImageRef {
        part,
        alt: alt_text(pic),
    }))
}

/// Read the content of a graphic frame (`a:graphic`, or an element holding
/// one): a table, chart, SmartArt diagram, embedded object or picture.
pub(crate) fn graphic(
    cx: &mut Context<'_>,
    rels: &Relationships,
    frame: &Element,
    location: &str,
    out: &mut Vec<Block>,
) {
    let Some(data) = frame.find("a:graphicData") else {
        return;
    };
    for item in data.children() {
        graphic_item(cx, rels, item, location, out);
    }
}

/// Read one item of an `a:graphicData`.
pub(crate) fn graphic_item(
    cx: &mut Context<'_>,
    rels: &Relationships,
    item: &Element,
    location: &str,
    out: &mut Vec<Block>,
) {
    match item.name() {
        "a:tbl" => out.push(Block::Table(table(rels, item))),
        "c:chart" => out.extend(chart_part(cx, rels, item.attr("r:id"), false)),
        "cx:chart" => out.extend(chart_part(cx, rels, item.attr("r:id"), true)),
        "dgm:relIds" => out.extend(smartart_part(cx, rels, item.attr("r:dm"))),
        "p:oleObj" => {
            let id = item.attr("r:id");
            let prog_id = item.attr("progId");
            if let Some(part) = id.and_then(|id| cx.object(rels, id, prog_id, location)) {
                out.push(Block::Embed(EmbedRef { part }));
            }
        }
        "pic:pic" => out.extend(picture(cx, rels, item, location)),
        _ => {}
    }
}

fn chart_part(
    cx: &mut Context<'_>,
    rels: &Relationships,
    id: Option<&str>,
    chartex: bool,
) -> Option<Block> {
    let target = rels.target(id?)?.to_string();
    let root = cx.xml(&target)?;
    if chartex {
        chart::read_chartex(&root)
    } else {
        chart::read(&root)
    }
}

fn smartart_part(cx: &mut Context<'_>, rels: &Relationships, id: Option<&str>) -> Vec<Block> {
    let Some(target) = id.and_then(|id| rels.target(id)).map(str::to_string) else {
        return Vec::new();
    };
    cx.xml(&target)
        .map(|root| smartart::read(&root))
        .unwrap_or_default()
}

/// Read a tree of shapes (`p:spTree`, `p:grpSp`, `xdr:grpSp`, ...) in
/// document order: text of shapes, pictures, graphic frames and groups.
/// Text of free-standing shapes is wrapped as a text box when it holds more
/// than one block, so it stays visually separate from surrounding text.
pub(crate) fn shape_tree(
    cx: &mut Context<'_>,
    rels: &Relationships,
    tree: &Element,
    location: &str,
    out: &mut Vec<Block>,
) {
    for child in tree.children() {
        shape(cx, rels, child, location, out);
    }
}

/// Read one shape of a shape tree.
pub(crate) fn shape(
    cx: &mut Context<'_>,
    rels: &Relationships,
    el: &Element,
    location: &str,
    out: &mut Vec<Block>,
) {
    match el.local() {
        "sp" => {
            let mut blocks = el
                .child_local("txBody")
                .map(|body| text_body(rels, body, Bullets::Explicit))
                .unwrap_or_default();
            if let Some(url) = click_link(rels, el) {
                link_shape(&mut blocks, url);
            }
            push_text_box(out, blocks);
        }
        "grpSp" => shape_tree(cx, rels, el, location, out),
        "pic" => out.extend(picture(cx, rels, el, location)),
        "graphicFrame" => graphic(cx, rels, el, location, out),
        _ => {}
    }
}

/// The external hyperlink followed when a shape is clicked.
fn click_link(rels: &Relationships, shape: &Element) -> Option<String> {
    let props = shape
        .children()
        .find(|c| c.local().starts_with("nv"))?
        .child_local("cNvPr")?;
    let id = props.child("a:hlinkClick")?.attr("r:id")?;
    Context::hyperlink(rels, id)
}
