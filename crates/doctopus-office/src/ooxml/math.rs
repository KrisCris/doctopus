//! Office Math (OMML) to LaTeX.
//!
//! Equations in Word and PowerPoint are stored as OMML trees. Agents read
//! LaTeX fluently, so the tree is converted to TeX: fractions, scripts,
//! radicals, n-ary operators, delimiters, functions, accents, matrices and
//! equation arrays are covered; anything else contributes its text.

use super::xml::{Element, Node};

/// Convert an `m:oMath` or `m:oMathPara` element.
pub(crate) fn to_tex(el: &Element) -> String {
    let mut tex = String::new();
    convert(el, &mut tex);
    tidy(&tex)
}

fn convert(el: &Element, out: &mut String) {
    match el.name() {
        "m:oMathPara" => {
            let lines: Vec<String> = el
                .children_named("m:oMath")
                .map(|m| {
                    let mut s = String::new();
                    convert(m, &mut s);
                    s
                })
                .collect();
            out.push_str(&lines.join(" \\\\ "));
        }
        "m:r" => {
            for t in el.children_named("m:t") {
                out.push_str(&escape(&t.text()));
            }
        }
        "m:t" => out.push_str(&escape(&el.text())),
        "m:f" => {
            let num = arg(el, "m:num");
            let den = arg(el, "m:den");
            match prop(el, "m:fPr", "m:type") {
                Some("lin") | Some("skw") => {
                    out.push_str(&format!("{}/{}", wrap(&num), wrap(&den)))
                }
                Some("noBar") => out.push_str(&format!("{{{num} \\atop {den}}}")),
                _ => out.push_str(&format!("\\frac{{{num}}}{{{den}}}")),
            }
        }
        "m:sSup" => out.push_str(&format!(
            "{}^{{{}}}",
            wrap(&arg(el, "m:e")),
            arg(el, "m:sup")
        )),
        "m:sSub" => out.push_str(&format!(
            "{}_{{{}}}",
            wrap(&arg(el, "m:e")),
            arg(el, "m:sub")
        )),
        "m:sSubSup" => out.push_str(&format!(
            "{}_{{{}}}^{{{}}}",
            wrap(&arg(el, "m:e")),
            arg(el, "m:sub"),
            arg(el, "m:sup")
        )),
        "m:sPre" => out.push_str(&format!(
            "{{}}_{{{}}}^{{{}}}{}",
            arg(el, "m:sub"),
            arg(el, "m:sup"),
            wrap(&arg(el, "m:e"))
        )),
        "m:rad" => {
            let deg = arg(el, "m:deg");
            let hidden = prop(el, "m:radPr", "m:degHide").is_some_and(on);
            if deg.trim().is_empty() || hidden {
                out.push_str(&format!("\\sqrt{{{}}}", arg(el, "m:e")));
            } else {
                out.push_str(&format!("\\sqrt[{deg}]{{{}}}", arg(el, "m:e")));
            }
        }
        "m:nary" => {
            let chr = prop(el, "m:naryPr", "m:chr").unwrap_or("\u{222B}");
            out.push_str(nary_command(chr));
            if !prop(el, "m:naryPr", "m:subHide").is_some_and(on) {
                let sub = arg(el, "m:sub");
                if !sub.is_empty() {
                    out.push_str(&format!("_{{{sub}}}"));
                }
            }
            if !prop(el, "m:naryPr", "m:supHide").is_some_and(on) {
                let sup = arg(el, "m:sup");
                if !sup.is_empty() {
                    out.push_str(&format!("^{{{sup}}}"));
                }
            }
            out.push(' ');
            out.push_str(&arg(el, "m:e"));
        }
        "m:d" => {
            let beg = prop(el, "m:dPr", "m:begChr").unwrap_or("(");
            let end = prop(el, "m:dPr", "m:endChr").unwrap_or(")");
            let sep = prop(el, "m:dPr", "m:sepChr").unwrap_or("|");
            let items: Vec<String> = el
                .children_named("m:e")
                .map(|e| {
                    let mut s = String::new();
                    convert(e, &mut s);
                    s
                })
                .collect();
            out.push_str(&format!(
                "\\left{} {} \\right{}",
                delimiter(beg),
                items.join(&format!(" {} ", escape(sep))),
                delimiter(end)
            ));
        }
        "m:func" => {
            let name = arg(el, "m:fName");
            let trimmed = name.trim();
            if KNOWN_FUNCTIONS.contains(&trimmed) {
                out.push_str(&format!("\\{trimmed} "));
            } else if trimmed.chars().all(char::is_alphabetic) && !trimmed.is_empty() {
                out.push_str(&format!("\\operatorname{{{trimmed}}} "));
            } else {
                out.push_str(&format!("{name} "));
            }
            out.push_str(&wrap(&arg(el, "m:e")));
        }
        "m:acc" => {
            let chr = prop(el, "m:accPr", "m:chr").unwrap_or("\u{0302}");
            out.push_str(&format!("{}{{{}}}", accent_command(chr), arg(el, "m:e")));
        }
        "m:bar" => {
            let top = prop(el, "m:barPr", "m:pos") == Some("top");
            let cmd = if top { "\\overline" } else { "\\underline" };
            out.push_str(&format!("{cmd}{{{}}}", arg(el, "m:e")));
        }
        "m:groupChr" => {
            let top = prop(el, "m:groupChrPr", "m:pos") == Some("top");
            let cmd = if top { "\\overbrace" } else { "\\underbrace" };
            out.push_str(&format!("{cmd}{{{}}}", arg(el, "m:e")));
        }
        "m:limLow" => {
            let base = arg(el, "m:e");
            let lim = arg(el, "m:lim");
            if base.trim() == "lim" {
                out.push_str(&format!("\\lim_{{{lim}}}"));
            } else {
                out.push_str(&format!("{}_{{{lim}}}", wrap(&base)));
            }
        }
        "m:limUpp" => out.push_str(&format!(
            "{}^{{{}}}",
            wrap(&arg(el, "m:e")),
            arg(el, "m:lim")
        )),
        "m:m" => {
            let rows: Vec<String> = el
                .children_named("m:mr")
                .map(|row| {
                    row.children_named("m:e")
                        .map(|e| {
                            let mut s = String::new();
                            convert(e, &mut s);
                            s
                        })
                        .collect::<Vec<_>>()
                        .join(" & ")
                })
                .collect();
            out.push_str(&format!(
                "\\begin{{matrix}} {} \\end{{matrix}}",
                rows.join(" \\\\ ")
            ));
        }
        "m:eqArr" => {
            let rows: Vec<String> = el
                .children_named("m:e")
                .map(|e| {
                    let mut s = String::new();
                    convert(e, &mut s);
                    s
                })
                .collect();
            out.push_str(&format!(
                "\\begin{{aligned}} {} \\end{{aligned}}",
                rows.join(" \\\\ ")
            ));
        }
        name if name.ends_with("Pr") => {}
        name if name.starts_with("w:") => {}
        _ => {
            for node in el.nodes() {
                if let Node::Element(child) = node {
                    convert(child, out);
                }
            }
        }
    }
}

/// The converted content of a named child.
fn arg(el: &Element, name: &str) -> String {
    let mut s = String::new();
    if let Some(child) = el.child(name) {
        convert(child, &mut s);
    }
    s.trim().to_string()
}

/// A `m:val` attribute of a property element (`m:fPr/m:type/@m:val`).
fn prop<'e>(el: &'e Element, pr: &str, name: &str) -> Option<&'e str> {
    el.child(pr)?.child(name)?.attr("m:val")
}

fn on(v: &str) -> bool {
    matches!(v, "1" | "on" | "true")
}

/// Braces around multi-character operands.
fn wrap(s: &str) -> String {
    if s.chars().count() <= 1 {
        s.to_string()
    } else {
        format!("{{{s}}}")
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '{' | '}' | '%' | '#' | '&' | '$' | '_' => {
                out.push('\\');
                out.push(c);
            }
            '\\' => out.push_str("\\backslash "),
            '\u{2061}' | '\u{2062}' | '\u{2063}' | '\u{200B}' => {}
            c => out.push(c),
        }
    }
    out
}

fn delimiter(chr: &str) -> &str {
    match chr {
        "" => ".",
        "{" => "\\{",
        "}" => "\\}",
        "\u{2016}" => "\\|",
        "\u{27E8}" | "\u{2329}" => "\\langle",
        "\u{27E9}" | "\u{232A}" => "\\rangle",
        "\u{230A}" => "\\lfloor",
        "\u{230B}" => "\\rfloor",
        "\u{2308}" => "\\lceil",
        "\u{2309}" => "\\rceil",
        other => other,
    }
}

fn nary_command(chr: &str) -> &'static str {
    match chr {
        "\u{2211}" => "\\sum",
        "\u{220F}" => "\\prod",
        "\u{2210}" => "\\coprod",
        "\u{222C}" => "\\iint",
        "\u{222D}" => "\\iiint",
        "\u{222E}" => "\\oint",
        "\u{22C3}" => "\\bigcup",
        "\u{22C2}" => "\\bigcap",
        "\u{22C1}" => "\\bigvee",
        "\u{22C0}" => "\\bigwedge",
        _ => "\\int",
    }
}

fn accent_command(chr: &str) -> &'static str {
    match chr {
        "\u{0303}" => "\\tilde",
        "\u{0304}" | "\u{0305}" | "\u{00AF}" => "\\bar",
        "\u{20D7}" | "\u{20D1}" => "\\vec",
        "\u{0307}" => "\\dot",
        "\u{0308}" => "\\ddot",
        "\u{030C}" => "\\check",
        "\u{0301}" => "\\acute",
        "\u{0300}" => "\\grave",
        "\u{0306}" => "\\breve",
        _ => "\\hat",
    }
}

const KNOWN_FUNCTIONS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "sinh", "cosh", "tanh", "coth", "arcsin", "arccos",
    "arctan", "log", "ln", "lg", "exp", "lim", "max", "min", "sup", "inf", "det", "dim", "ker",
    "gcd", "deg", "arg", "Pr",
];

/// Collapse the doubled spaces the converter leaves behind.
fn tidy(tex: &str) -> String {
    tex.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooxml::xml::parse;

    fn tex(body: &str) -> String {
        let xml = format!(
            r#"<m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">{body}</m:oMath>"#
        );
        to_tex(&parse(xml.as_bytes()).root)
    }

    #[test]
    fn converts_common_structures() {
        assert_eq!(
            tex(
                "<m:f><m:num><m:r><m:t>a</m:t></m:r></m:num><m:den><m:r><m:t>b+1</m:t></m:r></m:den></m:f>"
            ),
            "\\frac{a}{b+1}"
        );
        assert_eq!(
            tex(
                "<m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup>"
            ),
            "x^{2}"
        );
        assert_eq!(
            tex(
                r#"<m:nary><m:naryPr><m:chr m:val="∑"/></m:naryPr><m:sub><m:r><m:t>i=1</m:t></m:r></m:sub><m:sup><m:r><m:t>n</m:t></m:r></m:sup><m:e><m:r><m:t>i</m:t></m:r></m:e></m:nary>"#
            ),
            "\\sum_{i=1}^{n} i"
        );
        assert_eq!(
            tex(
                "<m:rad><m:radPr><m:degHide m:val=\"1\"/></m:radPr><m:deg/><m:e><m:r><m:t>2</m:t></m:r></m:e></m:rad>"
            ),
            "\\sqrt{2}"
        );
        assert_eq!(
            tex("<m:d><m:e><m:r><m:t>a</m:t></m:r></m:e><m:e><m:r><m:t>b</m:t></m:r></m:e></m:d>"),
            "\\left( a | b \\right)"
        );
        assert_eq!(
            tex(
                "<m:func><m:fName><m:r><m:t>sin</m:t></m:r></m:fName><m:e><m:r><m:t>x</m:t></m:r></m:e></m:func>"
            ),
            "\\sin x"
        );
    }
}
