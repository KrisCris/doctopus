//! `doctopus read`: render a document and print it a page at a time, or
//! write it to a file with a map of where each section starts.

use std::fmt::Write as _;
use std::path::Path;

use doctopus_core::render::{
    self, Page, RenderOptions, Rendered, SectionMark, estimate_tokens, paginate, shell_quote,
};
use doctopus_core::{Address, Document, SectionKind, SectionSelector, Selection};

use crate::cli::{DEFAULT_EMBED_DEPTH, DEFAULT_MAX_DEPTH, Format, ReadArgs, stem};
use crate::extract;
use crate::failure::{Code, Failure};
use crate::load::{engine, load};
use crate::output::{note, print};
use crate::paths;

/// Lines of the `-o` section map listed before eliding the rest.
const MAX_LISTED_SECTIONS: usize = 300;
/// Deepest Markdown heading listed in the `-o` map of documents without
/// sheets, slides or pages.
const MAX_HEADING_LEVEL: usize = 3;

/// `doctopus read`.
pub(crate) fn run(args: &ReadArgs, max_depth: u32) -> Result<(), Failure> {
    let address = Address::parse(&args.address);
    let engine = engine(max_depth, args.formulas);
    let (mut document, _) = load(&engine, &address)?.into_document(&address)?;
    let selection = Selection {
        sections: match &args.sections {
            Some(list) => SectionSelector::parse_list(list)?,
            None => Vec::new(),
        },
        cells: args.range,
    };
    selection.apply(&mut document)?;

    let mut options = RenderOptions::new(address.clone());
    options.embed_depth = args.embed_depth;
    options.formulas = args.formulas;
    options.front_matter = !args.no_front_matter;
    if let Some(dir) = &args.extract {
        let report = extract::save(Some(&document), None, &address, dir)?;
        if !report.files.is_empty() {
            note(&format!(
                "doctopus: saved {} part(s) to {}; references point at the saved files",
                report.files.len(),
                shell_quote(&paths::shown(dir).display().to_string())
            ));
        }
        // Links are read from where the output is: the file written with
        // -o, or the current directory.
        let base = args
            .output
            .as_deref()
            .map(|file| file.parent().unwrap_or(Path::new("")));
        options.extracted = report
            .files
            .into_iter()
            .map(|saved| {
                let link = match base {
                    Some(base) => paths::link(base, &saved.file),
                    None => paths::slashes(&saved.file),
                };
                (saved.path, link)
            })
            .collect();
    }

    let rendered = match args.format {
        Format::Markdown => render::markdown(&document, &options),
        Format::Text => render::text(&document, &options),
        Format::Json => Rendered {
            text: render::json(&document, &options),
            marks: Vec::new(),
        },
    };
    let context = Context {
        args,
        address: &address,
        max_depth,
        noun: primary_kind(&document),
    };
    if let Some(path) = &args.output {
        return write_file(path, &rendered, &context);
    }
    if args.format == Format::Json {
        if args.offset > 0 {
            return Err(Failure::new(
                Code::Usage,
                "JSON output is not paged; select sheets, slides or pages, or write it to a file with -o",
            ));
        }
        return print(&rendered.text);
    }
    let page = paginate(&rendered.text, args.offset, args.max_chars);
    if args.offset > 0 && page.start >= page.total {
        return Err(Failure::new(
            Code::Usage,
            format!(
                "--offset {} is past the end of the output ({} characters)",
                args.offset, page.total
            ),
        ));
    }
    print(page.text)?;
    if page.start > 0 || page.is_truncated() {
        note(&page_note(&page, &rendered, &context));
    }
    Ok(())
}

/// What the continuation commands need to repeat the request.
struct Context<'a> {
    args: &'a ReadArgs,
    address: &'a Address,
    max_depth: u32,
    /// Kind of the document's sheets, slides or pages, if it has any.
    noun: Option<SectionKind>,
}

impl Context<'_> {
    /// The `doctopus read` command for this request with `extra` appended.
    fn command(&self, extra: &[String]) -> String {
        let args = self.args;
        let mut words = vec![
            "doctopus".to_string(),
            "read".into(),
            shell_quote(&self.address.to_string()),
        ];
        if let Some(sections) = &args.sections {
            let flag = match self.noun {
                Some(SectionKind::Sheet) => "--sheet",
                Some(SectionKind::Slide) => "--slide",
                Some(SectionKind::Page) => "--page",
                _ => "--section",
            };
            words.extend([flag.into(), shell_quote(sections)]);
        }
        if let Some(range) = &args.range {
            words.extend(["--range".into(), range.to_string()]);
        }
        if args.formulas {
            words.push("--formulas".into());
        }
        if args.format != Format::Markdown {
            words.extend(["--format".into(), args.format.name().into()]);
        }
        if args.embed_depth != DEFAULT_EMBED_DEPTH {
            words.extend(["--embed-depth".into(), args.embed_depth.to_string()]);
        }
        if self.max_depth != DEFAULT_MAX_DEPTH {
            words.extend(["--max-depth".into(), self.max_depth.to_string()]);
        }
        if args.no_front_matter {
            words.push("--no-front-matter".into());
        }
        if let Some(dir) = &args.extract {
            words.extend(["--extract".into(), shell_quote(&dir.display().to_string())]);
        }
        words.extend(extra.iter().cloned());
        words.join(" ")
    }
}

fn primary_kind(document: &Document) -> Option<SectionKind> {
    document.primary_sections().next().map(|s| s.kind)
}

/// The note printed after a page: what it covers and how to go on.
fn page_note(page: &Page<'_>, rendered: &Rendered, context: &Context<'_>) -> String {
    let mut out = format!(
        "-- doctopus: printed characters {}-{} of {} (about {} tokens in all)",
        page.start,
        page.end,
        page.total,
        estimate_tokens(&rendered.text)
    );
    if let Some(span) = coverage(&rendered.marks, page) {
        let _ = write!(out, ", {span}");
    } else if page.start > 0
        && let Some(heading) = heading_before(&rendered.text, page.start)
    {
        let _ = write!(out, ", starting under the heading \"{heading}\"");
    }
    out.push('\n');
    if page.is_truncated() {
        let extra = [
            "--offset".to_string(),
            page.end.to_string(),
            max_chars_flag(context.args.max_chars),
        ];
        let extra: Vec<String> = extra.into_iter().filter(|w| !w.is_empty()).collect();
        let _ = writeln!(out, "-- next page: {}", context.command(&extra));
        let file = format!(
            "{}.{}",
            stem(context.address),
            context.args.format.extension()
        );
        let _ = writeln!(
            out,
            "-- everything to a file, with the line where each part starts: {}",
            context.command(&["-o".into(), shell_quote(&file)])
        );
    } else {
        out.push_str("-- end of output\n");
    }
    out
}

/// `--max-chars N` when it differs from the default (and so must be
/// repeated), otherwise nothing.
fn max_chars_flag(max_chars: usize) -> String {
    if max_chars == crate::cli::DEFAULT_MAX_CHARS {
        String::new()
    } else {
        format!("--max-chars {max_chars}")
    }
}

/// The sections a page shows: `slide 3 "Plan" to slide 7 of 12 slides`.
fn coverage(marks: &[SectionMark], page: &Page<'_>) -> Option<String> {
    let first = marks
        .iter()
        .rev()
        .find(|m| m.offset <= page.start)
        .or_else(|| marks.first())
        .filter(|m| m.offset < page.end)?;
    let last = marks.iter().rev().find(|m| m.offset < page.end)?;
    let primary = marks.iter().filter(|m| m.kind.is_primary()).count();
    let mut span = if std::ptr::eq(first, last) {
        format!("within {}", first.label())
    } else {
        format!("from {} to {}", first.label(), last.label())
    };
    if primary > 1
        && let Some(mark) = marks.iter().find(|m| m.kind.is_primary())
    {
        let _ = write!(span, " ({primary} {}s in all)", mark.kind.noun());
    }
    Some(span)
}

/// The last Markdown heading that starts before character `offset`.
fn heading_before(text: &str, offset: usize) -> Option<String> {
    let end = text
        .char_indices()
        .nth(offset)
        .map_or(text.len(), |(i, _)| i);
    text[..end]
        .lines()
        .rev()
        .find_map(heading)
        .map(|(_, title)| title.to_string())
}

/// The level and title of a Markdown heading line.
fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|&b| b == b'#').count();
    let title = line[level..].strip_prefix(' ')?.trim();
    ((1..=6).contains(&level) && !title.is_empty()).then_some((level, title))
}

/// `-o FILE`: write everything and print where each part starts.
fn write_file(path: &Path, rendered: &Rendered, context: &Context<'_>) -> Result<(), Failure> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|err| Failure::write(parent, &err))?;
    }
    std::fs::write(path, &rendered.text).map_err(|err| Failure::write(path, &err))?;

    let text = &rendered.text;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "output: {}",
        shell_quote(&paths::shown(path).display().to_string())
    );
    let _ = writeln!(
        out,
        "size: {} characters, about {} tokens, {} lines",
        text.chars().count(),
        estimate_tokens(text),
        text.lines().count()
    );
    // Documents without sheets, slides or pages are mapped by headings.
    let by_headings = context.args.format == Format::Markdown
        && !rendered.marks.iter().any(|m| m.kind.is_primary());
    let entries = if by_headings {
        headings(text)
    } else {
        let offsets: Vec<usize> = rendered.marks.iter().map(|m| m.offset).collect();
        line_numbers(text, &offsets)
            .into_iter()
            .zip(&rendered.marks)
            .map(|(line, mark)| (line, mark.label()))
            .collect()
    };
    if !entries.is_empty() {
        let what = if by_headings { "headings" } else { "sections" };
        let _ = writeln!(out, "{what}: # line where each starts");
        for (line, label) in entries.iter().take(MAX_LISTED_SECTIONS) {
            let _ = writeln!(out, "  - {line}: {label}");
        }
        if entries.len() > MAX_LISTED_SECTIONS {
            let _ = writeln!(out, "  - ... {} more", entries.len() - MAX_LISTED_SECTIONS);
        }
    }
    print(&out)
}

/// Markdown headings up to [`MAX_HEADING_LEVEL`] with their line numbers.
fn headings(text: &str) -> Vec<(usize, String)> {
    let mut fenced = false;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if let Some((level, title)) = heading(line).filter(|(l, _)| *l <= MAX_HEADING_LEVEL) {
            out.push((i + 1, format!("{} {title}", "#".repeat(level))));
        }
    }
    out
}

/// The 1-based line of each character offset; `offsets` ascend.
fn line_numbers(text: &str, offsets: &[usize]) -> Vec<usize> {
    let mut out = Vec::with_capacity(offsets.len());
    let mut line = 1;
    let mut chars = text.chars();
    let mut position = 0;
    for &offset in offsets {
        while position < offset {
            match chars.next() {
                Some('\n') => line += 1,
                Some(_) => {}
                None => break,
            }
            position += 1;
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(kind: SectionKind, number: u32, offset: usize) -> SectionMark {
        SectionMark {
            kind,
            number: Some(number),
            name: None,
            offset,
        }
    }

    fn page(start: usize, end: usize, total: usize) -> Page<'static> {
        Page {
            text: "",
            start,
            end,
            total,
        }
    }

    #[test]
    fn coverage_names_the_sections_on_a_page() {
        let marks = [
            mark(SectionKind::Slide, 1, 50),
            mark(SectionKind::Slide, 2, 400),
            mark(SectionKind::Slide, 3, 900),
        ];
        assert_eq!(
            coverage(&marks, &page(0, 500, 1000)).unwrap(),
            "from slide 1 to slide 2 (3 slides in all)"
        );
        assert_eq!(
            coverage(&marks, &page(500, 800, 1000)).unwrap(),
            "within slide 2 (3 slides in all)"
        );
        assert_eq!(coverage(&marks, &page(0, 40, 1000)), None);
    }

    #[test]
    fn headings_skip_code_blocks() {
        let text = "# Title\n\ntext\n```\n# not a heading\n```\n## Part\n#### deep\n";
        assert_eq!(
            headings(text),
            [(1, "# Title".to_string()), (7, "## Part".to_string())]
        );
        assert_eq!(heading_before(text, text.len()).as_deref(), Some("deep"));
    }

    #[test]
    fn offsets_become_line_numbers() {
        let text = "a\nbb\nccc\n";
        assert_eq!(line_numbers(text, &[0, 2, 5, 9]), [1, 2, 3, 4]);
    }
}
