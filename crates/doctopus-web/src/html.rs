//! Markdown files of a conversion as HTML for the page's viewer.
//!
//! The Markdown comes from untrusted documents, so the HTML carries no
//! markup from it beyond what the renderer itself writes (`<br>`, `<sup>`,
//! `<sub>`); any other HTML is shown as text. Links keep only web and mail
//! addresses, and nothing is loaded from elsewhere:
//!
//! - a link to a file of the conversion points at the page's own address
//!   for it, `#/` and the file's path relative to the output directory;
//! - a picture of the conversion gets `data-path` with that path instead of
//!   a source, for the page to fill in;
//! - a picture on the web becomes a link to it;
//! - links to anything else stay text, with the target as a tooltip;
//! - Mermaid blocks become `<pre class="mermaid">` for the page to draw.

use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd, html};

/// The Markdown file at `path` (relative to the output directory) as HTML.
/// `exists` says whether a path is a file of the conversion.
pub(crate) fn to_html(markdown: &str, path: &str, exists: impl Fn(&str) -> bool) -> String {
    let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut parser = Parser::new_ext(markdown, options);
    let mut events = Vec::new();
    // The closing tag of each open link.
    let mut open_links = Vec::new();
    while let Some(event) = parser.next() {
        let event = match event {
            Event::Start(Tag::Link {
                dest_url, title, ..
            }) => {
                let (open, close) = link(&target(&dest_url, dir, &exists), &title);
                open_links.push(close);
                Event::InlineHtml(open.into())
            }
            Event::End(TagEnd::Link) => {
                Event::InlineHtml(open_links.pop().unwrap_or_default().into())
            }
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                let alt = alt_text(&mut parser);
                Event::InlineHtml(image(&target(&dest_url, dir, &exists), &title, &alt).into())
            }
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(lang)))
                if lang.as_ref() == "mermaid" =>
            {
                let source = text_until(&mut parser, |end| matches!(end, TagEnd::CodeBlock));
                Event::Html(format!("<pre class=\"mermaid\">{}</pre>\n", escape(&source)).into())
            }
            Event::Start(Tag::MetadataBlock(_)) => {
                let yaml = text_until(&mut parser, |end| matches!(end, TagEnd::MetadataBlock(_)));
                Event::Html(
                    format!(
                        "<details class=\"front-matter\"><summary>YAML header</summary>\
                         <pre><code>{}</code></pre></details>\n",
                        escape(&yaml)
                    )
                    .into(),
                )
            }
            Event::Html(markup) | Event::InlineHtml(markup) => allowed(markup),
            other => other,
        };
        events.push(event);
    }
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    out
}

/// Where a link or picture leads.
#[derive(Debug, PartialEq)]
enum Target {
    /// A file of the conversion, by its path.
    Local(String),
    /// A web or mail address.
    Web(String),
    /// A place in the same file.
    Anchor(String),
    /// Anything else: a file that is not part of the conversion, or a
    /// scheme the page does not follow.
    Elsewhere(String),
}

/// Classify the link destination `url` of a Markdown file in `dir`.
fn target(url: &str, dir: &str, exists: &impl Fn(&str) -> bool) -> Target {
    if url.starts_with('#') {
        return Target::Anchor(url.to_string());
    }
    if let Some(scheme) = scheme(url) {
        return if ["http", "https", "mailto"]
            .iter()
            .any(|s| s.eq_ignore_ascii_case(scheme))
        {
            Target::Web(url.to_string())
        } else {
            Target::Elsewhere(url.to_string())
        };
    }
    // The renderer writes paths as they are; percent-encoded ones come from
    // the document.
    [url.to_string(), percent_decoded(url)]
        .iter()
        .filter_map(|url| resolve(dir, url))
        .find(|path| exists(path))
        .map_or_else(|| Target::Elsewhere(url.to_string()), Target::Local)
}

/// The scheme of an absolute URL: `https` of `https://example.com`.
fn scheme(url: &str) -> Option<&str> {
    let (scheme, _) = url.split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then_some(scheme)
}

/// The relative path `url` from the directory `dir`, both relative to the
/// output directory; `None` when it leads out of it.
fn resolve(dir: &str, url: &str) -> Option<String> {
    if url.starts_with(['/', '\\']) {
        return None;
    }
    let mut out: Vec<&str> = dir.split('/').filter(|c| !c.is_empty()).collect();
    for component in url.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            name => out.push(name),
        }
    }
    (!out.is_empty()).then(|| out.join("/"))
}

/// `url` with `%XX` escapes decoded, when they form UTF-8.
fn percent_decoded(url: &str) -> String {
    let bytes = url.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| url.to_string())
}

/// The page's address for the file at `path`: `#/` and the path, with
/// everything but letters, digits, `/` and `-._~` percent-encoded.
fn page_address(path: &str) -> String {
    let mut out = String::from("#/");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The opening and closing tags of a link to `target`.
fn link(target: &Target, title: &str) -> (String, &'static str) {
    let title = if title.is_empty() {
        String::new()
    } else {
        format!(" title=\"{}\"", escape(title))
    };
    match target {
        Target::Local(path) => (
            format!(
                "<a href=\"{}\" data-path=\"{}\"{title}>",
                escape(&page_address(path)),
                escape(path)
            ),
            "</a>",
        ),
        Target::Web(url) => (
            format!(
                "<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\"{title}>",
                escape(url)
            ),
            "</a>",
        ),
        Target::Anchor(anchor) => (format!("<a href=\"{}\"{title}>", escape(anchor)), "</a>"),
        Target::Elsewhere(url) => (
            format!(
                "<span class=\"elsewhere\" title=\"Not in this conversion: {}\">",
                escape(url)
            ),
            "</span>",
        ),
    }
}

/// A picture showing `target`.
fn image(target: &Target, title: &str, alt: &str) -> String {
    let title = if title.is_empty() {
        String::new()
    } else {
        format!(" title=\"{}\"", escape(title))
    };
    let label = escape(if alt.is_empty() { "picture" } else { alt });
    match target {
        Target::Local(path) => format!(
            "<img data-path=\"{}\" alt=\"{}\"{title}>",
            escape(path),
            escape(alt)
        ),
        Target::Web(url) => format!(
            "<a class=\"remote-image\" href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\"{title}>{label}</a>",
            escape(url)
        ),
        Target::Anchor(url) | Target::Elsewhere(url) => format!(
            "<span class=\"elsewhere\" title=\"Not in this conversion: {}\">{label}</span>",
            escape(url)
        ),
    }
}

/// The text of a picture's description, up to the end of the picture.
fn alt_text(parser: &mut Parser<'_>) -> String {
    let mut depth = 0;
    text_until(parser, |end| {
        if matches!(end, TagEnd::Image) {
            if depth == 0 {
                return true;
            }
            depth -= 1;
        }
        false
    })
}

/// The text of the events up to the end tag that `is_end` accepts.
fn text_until(parser: &mut Parser<'_>, mut is_end: impl FnMut(&TagEnd) -> bool) -> String {
    let mut text = String::new();
    for event in parser.by_ref() {
        match event {
            Event::End(end) if is_end(&end) => break,
            Event::Text(t) | Event::Code(t) => text.push_str(&t),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            _ => {}
        }
    }
    text
}

/// The renderer's own markup as it is, any other HTML as text.
fn allowed(markup: CowStr<'_>) -> Event<'_> {
    const ALLOWED: [&str; 7] = [
        "<br>", "<br/>", "<br />", "<sup>", "</sup>", "<sub>", "</sub>",
    ];
    if ALLOWED
        .iter()
        .any(|tag| tag.eq_ignore_ascii_case(markup.trim()))
    {
        Event::InlineHtml(markup)
    } else {
        Event::Text(markup)
    }
}

/// `text` safe inside an HTML element or a quoted attribute.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(markdown: &str) -> String {
        let files = [
            "report.md",
            "report.parts/emb1/Budget (draft).md",
            "report.parts/a b.png",
        ];
        to_html(markdown, "report.md", |p| files.contains(&p))
    }

    #[test]
    fn links_to_files_of_the_conversion_open_them_in_the_page() {
        let html = render("[Budget](<report.parts/emb1/Budget (draft).md>)");
        assert!(
            html.contains(
                "<a href=\"#/report.parts/emb1/Budget%20%28draft%29.md\" \
                 data-path=\"report.parts/emb1/Budget (draft).md\">Budget</a>"
            ),
            "{html}"
        );
        let html = render("![A, \"B\"](<report.parts/a b.png> \"Logo\")");
        assert!(
            html.contains(
                "<img data-path=\"report.parts/a b.png\" alt=\"A, &quot;B&quot;\" title=\"Logo\">"
            ),
            "{html}"
        );
        // Relative to the file that holds the link.
        let budget = to_html(
            "[up](../../report.md)",
            "report.parts/emb1/Budget.md",
            |p| p == "report.md",
        );
        assert!(budget.contains("href=\"#/report.md\""), "{budget}");
        assert!(
            render("[x](report.parts/a%20b.png)").contains("data-path=\"report.parts/a b.png\"")
        );
    }

    #[test]
    fn nothing_runs_and_nothing_is_fetched() {
        let html = render(
            "<script>alert(1)</script>\n\n\
             a<sup>2</sup> <img src=x onerror=alert(1)>\n\n\
             [x](javascript:alert(1)) [y](other.docx) [z](../outside.md) [w](/etc/passwd)\n\n\
             ![remote](https://example.com/t.png) [web](https://example.com \"T\")",
        );
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("<img src"), "{html}");
        assert!(html.contains("a<sup>2</sup>"), "{html}");
        assert!(!html.contains("href=\"javascript"), "{html}");
        assert!(
            html.contains(
                "<span class=\"elsewhere\" title=\"Not in this conversion: other.docx\">y</span>"
            ),
            "{html}"
        );
        assert!(
            html.contains("Not in this conversion: ../outside.md"),
            "{html}"
        );
        assert!(
            html.contains("Not in this conversion: /etc/passwd"),
            "{html}"
        );
        assert!(
            html.contains("<a class=\"remote-image\" href=\"https://example.com/t.png\""),
            "{html}"
        );
        assert!(
            html.contains(
                "<a href=\"https://example.com\" target=\"_blank\" rel=\"noopener noreferrer\" title=\"T\">web</a>"
            ),
            "{html}"
        );
    }

    #[test]
    fn diagrams_headers_and_tables_are_kept() {
        let html = render(
            "---\nsource: \"a<b>.docx\"\n---\n\n| a | b |\n|---|---|\n| 1<br>2 | 3 |\n\n\
             ```mermaid\nflowchart TD\n    n1[\"x\"] -->|\"a&b\"| n2\n```\n",
        );
        assert!(
            html.contains(
                "<summary>YAML header</summary><pre><code>source: &quot;a&lt;b&gt;.docx&quot;"
            ),
            "{html}"
        );
        assert!(html.contains("<td>1<br>2</td>"), "{html}");
        assert!(
            html.contains("<pre class=\"mermaid\">flowchart TD\n    n1[&quot;x&quot;] --&gt;|&quot;a&amp;b&quot;| n2\n</pre>"),
            "{html}"
        );
    }

    #[test]
    fn paths_resolve_inside_the_output_directory() {
        assert_eq!(resolve("a/b", "../c.md"), Some("a/c.md".into()));
        assert_eq!(resolve("", "./x/./y.png"), Some("x/y.png".into()));
        assert_eq!(resolve("a", "../../x"), None);
        assert_eq!(scheme("https://x"), Some("https"));
        assert_eq!(scheme("C:\\x"), Some("C"));
        assert_eq!(scheme("report.parts/a:b.png"), None);
        assert_eq!(percent_decoded("a%20b%zz%E2%82%AC"), "a b%zz€");
        assert_eq!(page_address("a b/é.md"), "#/a%20b/%C3%A9.md");
    }
}
