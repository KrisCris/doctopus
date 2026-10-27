//! Paragraph styles (for headings, list membership, code and quotations),
//! character styles, and list numbering definitions with the counters that
//! turn them into labels.

use std::collections::HashMap;

use crate::fonts::is_monospace;
use crate::numbering::{CounterStyle, Instance, Level, NumRef, Numbering};
use crate::ooxml::xml::Element;
use crate::word::{Role, role_by_name};

/// How far `w:basedOn` chains are followed; real documents use a handful,
/// the limit only guards against cycles.
const MAX_INHERITANCE: usize = 16;

#[derive(Debug, Default)]
struct ParagraphStyle {
    name: String,
    based_on: Option<String>,
    outline: Option<u8>,
    num: Option<NumRef>,
    /// Whether the style's font is fixed-width, when it names one.
    mono: Option<bool>,
}

#[derive(Debug, Default)]
struct CharacterStyle {
    based_on: Option<String>,
    bold: Option<bool>,
    italic: Option<bool>,
    mono: Option<bool>,
    /// Named for code (`HTML Code`, `Verbatim Char`).
    code: bool,
}

/// Formatting a character style sets.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct CharFormat {
    pub(crate) bold: Option<bool>,
    pub(crate) italic: Option<bool>,
    pub(crate) code: Option<bool>,
}

impl NumRef {
    /// Read a `w:numPr`.
    pub(crate) fn parse(num_pr: &Element) -> Option<Self> {
        let num_id = num_pr.child_val("w:numId").map(str::to_string);
        let level = num_pr.child_val("w:ilvl").and_then(|v| v.parse().ok());
        match num_id {
            Some(num_id) => Some(Self { num_id, level }),
            // A level without an id inherits the id from the style.
            None => level.map(|level| Self {
                num_id: String::new(),
                level: Some(level),
            }),
        }
    }
}

/// The styles part.
#[derive(Debug, Default)]
pub(crate) struct Styles {
    paragraph: HashMap<String, ParagraphStyle>,
    character: HashMap<String, CharacterStyle>,
    default_paragraph: Option<String>,
    /// Whether the document's default font is fixed-width.
    mono_default: bool,
}

impl Styles {
    pub(crate) fn parse(root: &Element) -> Self {
        let mut styles = Self::default();
        for style in root.children_named("w:style") {
            let Some(id) = style.attr("w:styleId") else {
                continue;
            };
            let based_on = style.child_val("w:basedOn").map(str::to_string);
            match style.attr("w:type").unwrap_or("paragraph") {
                "paragraph" => {
                    let ppr = style.child("w:pPr");
                    let paragraph = ParagraphStyle {
                        name: style.child_val("w:name").unwrap_or_default().to_string(),
                        based_on,
                        outline: ppr
                            .and_then(|p| p.child_val("w:outlineLvl"))
                            .and_then(|v| v.parse().ok()),
                        num: ppr.and_then(|p| p.child("w:numPr")).and_then(NumRef::parse),
                        mono: mono(style.child("w:rPr")),
                    };
                    if style.attr_bool("w:default") == Some(true) {
                        styles.default_paragraph = Some(id.to_string());
                    }
                    styles.paragraph.insert(id.to_string(), paragraph);
                }
                "character" => {
                    let rpr = style.child("w:rPr");
                    let name = style.child_val("w:name").unwrap_or_default();
                    let name = name.to_ascii_lowercase();
                    let character = CharacterStyle {
                        based_on,
                        bold: rpr.and_then(|r| r.child("w:b")).map(toggle),
                        italic: rpr.and_then(|r| r.child("w:i")).map(toggle),
                        mono: mono(rpr),
                        code: ["code", "verbatim", "keyboard", "typewriter"]
                            .iter()
                            .any(|w| name.contains(w)),
                    };
                    styles.character.insert(id.to_string(), character);
                }
                _ => {}
            }
        }
        let defaults = root
            .child("w:docDefaults")
            .and_then(|d| d.child("w:rPrDefault"))
            .and_then(|r| r.child("w:rPr"));
        styles.mono_default = mono(defaults).unwrap_or(false);
        styles
    }

    /// Whether the text of a paragraph style is fixed-width. Text in a
    /// fixed-width font is code only where the paragraph's is not.
    pub(crate) fn paragraph_mono(&self, id: Option<&str>) -> bool {
        let mut current = id;
        for _ in 0..MAX_INHERITANCE {
            let Some(style) = current.and_then(|id| self.paragraph.get(id)) else {
                break;
            };
            if let Some(mono) = style.mono {
                return mono;
            }
            current = style.based_on.as_deref();
        }
        self.mono_default
    }

    /// Whether a run's `w:rFonts` is fixed-width: `None` when it names no
    /// font.
    pub(crate) fn font_mono(fonts: &Element) -> Option<bool> {
        font_mono(fonts)
    }

    /// Whether a paragraph style holds code or quotations, by its name or
    /// the name of a style it is based on. Fonts do not count: prose is
    /// written in the fixed-width `Plain Text` style too.
    pub(crate) fn role(&self, id: &str) -> Role {
        let mut current = Some(id);
        for _ in 0..MAX_INHERITANCE {
            let Some(style) = current.and_then(|id| self.paragraph.get(id)) else {
                break;
            };
            if let Some(role) = role_by_name(&style.name) {
                return role;
            }
            current = style.based_on.as_deref();
        }
        Role::Text
    }

    /// The paragraph style in effect (the given one, else the default).
    pub(crate) fn effective<'s>(&'s self, id: Option<&'s str>) -> Option<&'s str> {
        id.or(self.default_paragraph.as_deref())
    }

    /// The heading level a paragraph style gives, if it is a heading.
    pub(crate) fn heading_level(&self, id: &str) -> Option<u8> {
        let mut current = Some(id);
        for _ in 0..MAX_INHERITANCE {
            let style = self.paragraph.get(current?)?;
            let name = style.name.to_ascii_lowercase();
            if let Some(n) = name
                .strip_prefix("heading ")
                .and_then(|n| n.trim().parse::<u8>().ok())
            {
                return Some(n.clamp(1, 9));
            }
            if name == "title" {
                return Some(1);
            }
            if let Some(outline) = style.outline {
                return (outline < 9).then_some(outline + 1);
            }
            current = style.based_on.as_deref();
        }
        None
    }

    /// The list a paragraph style puts its paragraphs in.
    pub(crate) fn numbering(&self, id: &str) -> Option<NumRef> {
        let mut current = Some(id);
        let mut found: Option<NumRef> = None;
        for _ in 0..MAX_INHERITANCE {
            let Some(style) = current.and_then(|id| self.paragraph.get(id)) else {
                break;
            };
            if let Some(num) = &style.num {
                match &mut found {
                    None => found = Some(num.clone()),
                    // Fill in what the more specific style left out.
                    Some(f) => {
                        if f.num_id.is_empty() {
                            f.num_id.clone_from(&num.num_id);
                        }
                        if f.level.is_none() {
                            f.level = num.level;
                        }
                    }
                }
                if found
                    .as_ref()
                    .is_some_and(|f| !f.num_id.is_empty() && f.level.is_some())
                {
                    break;
                }
            }
            current = style.based_on.as_deref();
        }
        found.filter(|f| !f.num_id.is_empty())
    }

    /// Bold, italic and code set by a character style.
    pub(crate) fn character(&self, id: &str) -> CharFormat {
        let mut current = Some(id);
        let mut format = CharFormat::default();
        for _ in 0..MAX_INHERITANCE {
            let Some(style) = current.and_then(|id| self.character.get(id)) else {
                break;
            };
            format.bold = format.bold.or(style.bold);
            format.italic = format.italic.or(style.italic);
            format.code = format.code.or(style.code.then_some(true)).or(style.mono);
            current = style.based_on.as_deref();
        }
        format
    }
}

/// Whether the font of an `w:rPr` is fixed-width, when it names one.
fn mono(rpr: Option<&Element>) -> Option<bool> {
    font_mono(rpr?.child("w:rFonts")?)
}

fn font_mono(fonts: &Element) -> Option<bool> {
    match fonts.attr("w:ascii").or_else(|| fonts.attr("w:hAnsi")) {
        Some(name) => Some(is_monospace(name)),
        // Theme fonts are the document's body and heading fonts.
        None => fonts.attr("w:asciiTheme").map(|_| false),
    }
}

/// An `ST_OnOff` toggle element such as `<w:b/>` or `<w:b w:val="0"/>`.
pub(crate) fn toggle(el: &Element) -> bool {
    el.attr_bool("w:val").unwrap_or(true)
}

impl Numbering {
    pub(crate) fn parse(root: &Element, styles: &Styles) -> Self {
        let mut numbering = Self::default();
        let mut style_links: HashMap<String, String> = HashMap::new();
        let mut num_style_links: Vec<(String, String)> = Vec::new();
        for abs in root.children_named("w:abstractNum") {
            let Some(id) = abs.attr("w:abstractNumId") else {
                continue;
            };
            if let Some(link) = abs.child_val("w:styleLink") {
                style_links.insert(link.to_string(), id.to_string());
            }
            if let Some(link) = abs.child_val("w:numStyleLink") {
                num_style_links.push((id.to_string(), link.to_string()));
            }
            let mut levels = vec![Level::default(); 9];
            for lvl in abs.children_named("w:lvl") {
                if let Some(slot) = lvl
                    .attr_parse::<usize>("w:ilvl")
                    .and_then(|i| levels.get_mut(i))
                {
                    *slot = parse_level(lvl);
                }
            }
            numbering.abstracts.insert(id.to_string(), levels);
        }
        for num in root.children_named("w:num") {
            let Some(id) = num.attr("w:numId") else {
                continue;
            };
            let mut instance = Instance {
                abstract_id: num
                    .child_val("w:abstractNumId")
                    .unwrap_or_default()
                    .to_string(),
                ..Instance::default()
            };
            for ov in num.children_named("w:lvlOverride") {
                let Some(level) = ov.attr_parse::<u8>("w:ilvl") else {
                    continue;
                };
                if let Some(start) = ov.child_val("w:startOverride").and_then(|v| v.parse().ok()) {
                    instance.start_overrides.insert(level, start);
                }
                if let Some(lvl) = ov.child("w:lvl") {
                    instance.level_overrides.insert(level, parse_level(lvl));
                }
            }
            numbering.instances.insert(id.to_string(), instance);
        }
        // Abstract definitions that defer to a numbering style use the
        // definition that style links to.
        for (abstract_id, style_id) in num_style_links {
            let target = style_links.get(&style_id).cloned().or_else(|| {
                let num = styles.numbering(&style_id)?;
                Some(numbering.instances.get(&num.num_id)?.abstract_id.clone())
            });
            if let Some(levels) = target.and_then(|t| numbering.abstracts.get(&t).cloned()) {
                numbering.abstracts.insert(abstract_id, levels);
            }
        }
        numbering
    }
}

fn parse_level(lvl: &Element) -> Level {
    Level {
        // An omitted start is zero (ECMA-376 17.9.25); Word leaves it out
        // for lists that count from 0.
        start: lvl
            .child_val("w:start")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        style: CounterStyle::from_word(lvl.child_val("w:numFmt").unwrap_or("decimal")),
        text: lvl.child_val("w:lvlText").unwrap_or_default().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::numbering::{Counters, ListLabel};
    use crate::ooxml::xml::parse;

    const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

    fn numbering_xml(body: &str) -> Element {
        parse(format!(r#"<w:numbering xmlns:w="{W}">{body}</w:numbering>"#).as_bytes()).root
    }

    #[test]
    fn headings_come_from_names_outline_levels_and_inheritance() {
        let xml = format!(
            r#"<w:styles xmlns:w="{W}">
              <w:style w:type="paragraph" w:styleId="berschrift1"><w:name w:val="heading 1"/></w:style>
              <w:style w:type="paragraph" w:styleId="Custom"><w:name w:val="My heading"/><w:pPr><w:outlineLvl w:val="2"/></w:pPr></w:style>
              <w:style w:type="paragraph" w:styleId="Derived"><w:name w:val="Derived"/><w:basedOn w:val="Custom"/></w:style>
              <w:style w:type="paragraph" w:styleId="Body"><w:name w:val="Body"/><w:pPr><w:outlineLvl w:val="9"/></w:pPr></w:style>
            </w:styles>"#
        );
        let styles = Styles::parse(&parse(xml.as_bytes()).root);
        assert_eq!(styles.heading_level("berschrift1"), Some(1));
        assert_eq!(styles.heading_level("Custom"), Some(3));
        assert_eq!(styles.heading_level("Derived"), Some(3));
        assert_eq!(styles.heading_level("Body"), None);
        assert_eq!(styles.heading_level("Missing"), None);
    }

    #[test]
    fn code_and_quotations_come_from_style_names_and_fonts() {
        let mono = r#"<w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas"/></w:rPr>"#;
        let xml = format!(
            r#"<w:styles xmlns:w="{W}">
              <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
              <w:style w:type="paragraph" w:styleId="SourceCode"><w:name w:val="Source Code"/></w:style>
              <w:style w:type="paragraph" w:styleId="MyCode"><w:name w:val="Mine"/><w:basedOn w:val="SourceCode"/></w:style>
              <w:style w:type="paragraph" w:styleId="IntenseQuote"><w:name w:val="Intense Quote"/></w:style>
              <w:style w:type="paragraph" w:styleId="PlainText"><w:name w:val="Plain Text"/>{mono}</w:style>
              <w:style w:type="character" w:styleId="VerbatimChar"><w:name w:val="Verbatim Char"/></w:style>
              <w:style w:type="character" w:styleId="Fixed"><w:name w:val="Fixed"/>{mono}</w:style>
              <w:style w:type="character" w:styleId="Strong"><w:name w:val="Strong"/><w:rPr><w:b/></w:rPr></w:style>
            </w:styles>"#
        );
        let styles = Styles::parse(&parse(xml.as_bytes()).root);
        assert_eq!(styles.role("SourceCode"), Role::Code);
        assert_eq!(styles.role("MyCode"), Role::Code);
        assert_eq!(styles.role("IntenseQuote"), Role::Quote);
        // A fixed-width paragraph style is prose; its runs are not code.
        assert_eq!(styles.role("PlainText"), Role::Text);
        assert!(styles.paragraph_mono(Some("PlainText")));
        assert!(!styles.paragraph_mono(Some("Normal")));
        assert_eq!(styles.character("VerbatimChar").code, Some(true));
        assert_eq!(styles.character("Fixed").code, Some(true));
        assert_eq!(styles.character("Strong").code, None);
    }

    #[test]
    fn counters_produce_multilevel_labels() {
        let root = numbering_xml(
            r#"<w:abstractNum w:abstractNumId="0">
                 <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl>
                 <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%1.%2)"/></w:lvl>
               </w:abstractNum>
               <w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/><w:lvlText w:val=""/></w:lvl></w:abstractNum>
               <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
               <w:num w:numId="2"><w:abstractNumId w:val="0"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/></w:lvlOverride></w:num>
               <w:num w:numId="3"><w:abstractNumId w:val="1"/></w:num>"#,
        );
        let numbering = Numbering::parse(&root, &Styles::default());
        let mut counters = Counters::default();
        let at = |id: &str, level: u8| NumRef {
            num_id: id.into(),
            level: Some(level),
        };
        let mut next = |id, level| counters.next(&numbering, &at(id, level));
        assert_eq!(next("1", 0), ListLabel::Number("1.".into()));
        assert_eq!(next("1", 1), ListLabel::Number("1.a)".into()));
        assert_eq!(next("1", 1), ListLabel::Number("1.b)".into()));
        assert_eq!(next("1", 0), ListLabel::Number("2.".into()));
        assert_eq!(next("1", 1), ListLabel::Number("2.a)".into()));
        assert_eq!(next("2", 0), ListLabel::Number("5.".into()));
        assert_eq!(next("3", 0), ListLabel::Bullet);
        assert_eq!(next("9", 0), ListLabel::None);
    }
}
