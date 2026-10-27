//! From glyphs placed on a page to lines, paragraphs and headings.
//!
//! PDF has no paragraphs, only glyphs drawn at positions. Content streams
//! nearly always draw text in reading order, so that order is kept and only
//! the breaks are inferred: a new line where the baseline moves, a space
//! where glyphs are far apart, and a new paragraph where the line spacing
//! grows, the font size changes, a line is indented, a list item starts or
//! the text moves to another column. Headings are paragraphs set clearly
//! larger than the body text. Lines repeated at the top or bottom of most
//! pages are running headers and footers.

use std::collections::{BTreeMap, BTreeSet};

use doctopus_core::{
    Block, ImageRef, Inline, Link, ListItem, PartId, Style, plain_text, push_text,
};

/// A glyph closer than this (in ems) to an identical glyph on its line is
/// the same glyph drawn again, for fake bold or fill then stroke.
const OVERPRINT: f64 = 0.15;
/// How many of the latest glyphs of a line are checked for that.
const OVERPRINT_WINDOW: usize = 256;
/// Glyphs further apart than this (in ems) are separate words.
const WORD_GAP: f64 = 0.15;
/// A glyph whose baseline is within this (in ems) of a line's is on it.
const BASELINE: f64 = 0.5;
/// Lines further apart than this many times the usual line spacing are
/// separate paragraphs, and never more than `MAX_SPACING` ems apart.
const PARAGRAPH_SPACING: f64 = 1.35;
const MAX_SPACING: f64 = 2.5;
/// A line starting this far (in ems) right of the line above starts a
/// paragraph, unless it is centered under it, to within `CENTERED` ems.
const INDENT: f64 = 0.8;
const CENTERED: f64 = 0.5;
/// Paragraphs set at least this much larger than the body are headings,
/// when they are short.
const HEADING_SCALE: f64 = 1.15;
const HEADING_CHARS: usize = 200;
const HEADING_LINES: usize = 4;
/// Running headers and footers sit in this fraction of the page height at
/// the top or bottom, and repeat on at least `RUNNING_PAGES` pages and
/// half of the pages with text.
const RUNNING_BAND: f64 = 0.1;
const RUNNING_PAGES: usize = 4;

/// One glyph as drawn.
#[derive(Debug, Clone)]
pub(crate) struct Glyph {
    /// Start of the glyph on its baseline, in points, in a frame turned so
    /// that the text runs left to right with y going up.
    pub(crate) x: f64,
    pub(crate) y: f64,
    /// Font size in points.
    pub(crate) size: f64,
    /// Advance width in points.
    pub(crate) width: f64,
    /// Direction of the baseline in quarter turns counterclockwise.
    pub(crate) turn: u8,
    /// The characters the glyph stands for.
    pub(crate) text: String,
    pub(crate) style: Style,
    /// The link covering the glyph, as an index into the page's links.
    pub(crate) link: Option<usize>,
}

/// What a page draws, in content-stream order.
#[derive(Debug)]
pub(crate) enum Mark {
    Glyph(Glyph),
    Image(PartId),
}

/// The text and pictures of one page, in reading order.
#[derive(Debug, Default)]
pub(crate) struct Page {
    pub(crate) pieces: Vec<Piece>,
    /// Characters by font size in half points.
    sizes: BTreeMap<u64, usize>,
    /// The top and bottom of the page, in points.
    top: f64,
    bottom: f64,
}

#[derive(Debug)]
pub(crate) enum Piece {
    Text(Text),
    Image(PartId),
}

/// A paragraph once laid out: its content and what decides how it is
/// shown.
#[derive(Debug)]
pub(crate) struct Text {
    content: Vec<Inline>,
    /// The size of its first line.
    size: f64,
    lines: usize,
    chars: usize,
    alphabetic: bool,
    top: f64,
    bottom: f64,
    turn: u8,
    x0: f64,
    /// The list marker it starts with.
    marker: Option<String>,
}

#[derive(Debug)]
struct Line {
    glyphs: Vec<Glyph>,
    /// Whether a space precedes each glyph.
    spaced: Vec<bool>,
    x0: f64,
    x1: f64,
    y: f64,
    /// The largest size on the line.
    max_size: f64,
    turn: u8,
}

impl Line {
    fn new(glyph: Glyph) -> Self {
        Self {
            x0: glyph.x,
            x1: glyph.x + glyph.width,
            y: glyph.y,
            max_size: glyph.size,
            turn: glyph.turn,
            spaced: vec![false],
            glyphs: vec![glyph],
        }
    }

    /// Add `glyph` if it continues this line. Returns it otherwise.
    fn extend(&mut self, glyph: Glyph, space: bool) -> Option<Glyph> {
        let size = glyph.size.max(self.max_size);
        if glyph.turn != self.turn || (glyph.y - self.y).abs() > BASELINE * size {
            return Some(glyph);
        }
        if self.overprinted_by(&glyph) {
            return None;
        }
        let gap = glyph.x - self.x1;
        if gap < -BASELINE * glyph.size {
            return Some(glyph);
        }
        let last = self.glyphs.last().map_or(glyph.size, |g| g.size);
        let spaced = space || gap > WORD_GAP * (glyph.size + last) / 2.0;
        self.x1 = self.x1.max(glyph.x + glyph.width);
        self.max_size = size;
        self.spaced.push(spaced);
        self.glyphs.push(glyph);
        None
    }

    fn overprinted_by(&self, glyph: &Glyph) -> bool {
        let near = OVERPRINT * glyph.size;
        self.glyphs.iter().rev().take(OVERPRINT_WINDOW).any(|g| {
            g.text == glyph.text && (g.x - glyph.x).abs() < near && (g.y - glyph.y).abs() < near
        })
    }

    /// The size most of the line's characters are set in.
    fn size(&self) -> f64 {
        let mut counts: BTreeMap<u64, usize> = BTreeMap::new();
        for g in &self.glyphs {
            *counts.entry(half_points(g.size)).or_default() += g.text.chars().count();
        }
        counts
            .iter()
            .max_by_key(|(size, n)| (**n, std::cmp::Reverse(**size)))
            .map_or(self.max_size, |(size, _)| *size as f64 / 2.0)
    }

    fn text(&self) -> String {
        let mut text = String::new();
        for (g, spaced) in self.glyphs.iter().zip(&self.spaced) {
            if *spaced {
                text.push(' ');
            }
            text.push_str(&g.text);
        }
        text
    }
}

/// A size rounded to half points, as a key.
fn half_points(size: f64) -> u64 {
    (size * 2.0).round().max(0.0) as u64
}

/// One paragraph: lines that belong together.
#[derive(Debug)]
struct Paragraph {
    lines: Vec<Line>,
    x0: f64,
    x1: f64,
    /// The size of its first line.
    size: f64,
    /// The list marker its first line starts with.
    marker: Option<String>,
}

impl Paragraph {
    fn new(line: Line) -> Self {
        let marker = list_marker(&line.text());
        Self {
            x0: line.x0,
            x1: line.x1,
            size: line.size(),
            marker,
            lines: vec![line],
        }
    }

    fn freeze(self, links: &[String]) -> Text {
        let glyphs = || self.lines.iter().flat_map(|l| &l.glyphs);
        Text {
            content: self.inlines(links),
            size: self.size,
            lines: self.lines.len(),
            chars: glyphs().map(|g| g.text.chars().count()).sum(),
            alphabetic: glyphs().any(|g| g.text.chars().any(char::is_alphabetic)),
            top: self.lines.first().map_or(0.0, |l| l.y + l.max_size),
            bottom: self.lines.last().map_or(0.0, |l| l.y),
            turn: self.lines.first().map_or(0, |l| l.turn),
            x0: self.x0,
            marker: self.marker,
        }
    }

    /// Whether `line` continues this paragraph, lines usually being
    /// `spacing` ems apart.
    fn continues(&self, line: &Line, spacing: f64) -> bool {
        let Some(prev) = self.lines.last() else {
            return false;
        };
        let (size, prev_size) = (line.size(), prev.size());
        let larger = size.max(prev_size);
        let drop = prev.y - line.y;
        line.turn == prev.turn
            && (size - prev_size).abs() <= 0.15 * larger
            && drop >= 0.5 * larger
            && drop <= (spacing * PARAGRAPH_SPACING).min(MAX_SPACING) * larger
            && line.x0 <= self.x1
            && line.x1 >= self.x0
            && list_marker(&line.text()).is_none()
            && (self.marker.is_some()
                || line.x0 <= prev.x0 + INDENT * size
                || (prev.x0 - self.x0).abs() > 0.5 * size
                || centered(prev, line))
    }

    fn push(&mut self, line: Line) {
        self.x0 = self.x0.min(line.x0);
        self.x1 = self.x1.max(line.x1);
        self.lines.push(line);
    }

    /// The paragraph's text with styles and links. Lines are joined with a
    /// space, without one between CJK characters, and a word broken by a
    /// hyphen at the end of a line is joined again.
    fn inlines(&self, links: &[String]) -> Vec<Inline> {
        let mut runs: Vec<Run> = Vec::new();
        for (n, line) in self.lines.iter().enumerate() {
            if n > 0 {
                let before = runs.last().map_or("", |r| r.text.as_str());
                let after = line.glyphs.first().and_then(|g| g.text.chars().next());
                match join(before, after) {
                    Join::Space => {
                        let style = line
                            .glyphs
                            .first()
                            .map_or(Style::default(), |g| between(&runs, g.style));
                        push_run(&mut runs, " ", style, None);
                    }
                    Join::Dehyphenate => {
                        if let Some(last) = runs.last_mut() {
                            last.text.pop();
                        }
                    }
                    Join::Touch => {}
                }
            }
            for (g, spaced) in line.glyphs.iter().zip(&line.spaced) {
                if *spaced {
                    // A space inside a link belongs to it.
                    let link = runs
                        .last()
                        .and_then(|r| r.link)
                        .filter(|l| Some(*l) == g.link);
                    let style = if link.is_some() {
                        g.style
                    } else {
                        between(&runs, g.style)
                    };
                    push_run(&mut runs, " ", style, link);
                }
                push_run(&mut runs, &g.text, g.style, g.link);
            }
        }
        let mut content: Vec<Inline> = Vec::new();
        for run in runs {
            let Some(url) = run.link.and_then(|l| links.get(l)) else {
                push_text(&mut content, &run.text, run.style);
                continue;
            };
            match content.last_mut() {
                Some(Inline::Link(last)) if last.url == *url => {
                    push_text(&mut last.content, &run.text, run.style);
                }
                _ => {
                    let mut inner = Vec::new();
                    push_text(&mut inner, &run.text, run.style);
                    content.push(Inline::Link(Link {
                        url: url.clone(),
                        content: inner,
                    }));
                }
            }
        }
        content
    }
}

/// Whether `line` is centered under `above` and shorter on both sides, as
/// the lines of a centered title are. An indented first line reaches the
/// right margin instead.
fn centered(above: &Line, line: &Line) -> bool {
    let size = line.size();
    line.x1 < above.x1 - INDENT * size
        && ((line.x0 + line.x1) - (above.x0 + above.x1)).abs() <= 2.0 * CENTERED * size
}

/// The style of a space between the last run and text in `next` style:
/// theirs when they share it, so that emphasis spans whole phrases.
fn between(runs: &[Run], next: Style) -> Style {
    match runs.last() {
        Some(run) if run.style == next => next,
        _ => Style::default(),
    }
}

/// Text in one style, under one link.
struct Run {
    text: String,
    style: Style,
    link: Option<usize>,
}

fn push_run(runs: &mut Vec<Run>, text: &str, style: Style, link: Option<usize>) {
    match runs.last_mut() {
        Some(last) if last.style == style && last.link == link => last.text.push_str(text),
        _ => runs.push(Run {
            text: text.to_string(),
            style,
            link,
        }),
    }
}

enum Join {
    Space,
    Touch,
    Dehyphenate,
}

/// How a line ending in `before` joins the next, starting with `after`.
fn join(before: &str, after: Option<char>) -> Join {
    let mut end = before.chars().rev();
    let (last, previous) = (end.next(), end.next());
    match (last, after) {
        (None, _) | (_, None) => Join::Touch,
        (Some(b), _) if b.is_whitespace() => Join::Touch,
        (Some('\u{AD}'), _) => Join::Dehyphenate,
        (Some('-' | '\u{2010}'), Some(a))
            if previous.is_some_and(char::is_alphabetic) && a.is_lowercase() =>
        {
            Join::Dehyphenate
        }
        // A compound broken at its hyphen: "Stress-" and "Engineered",
        // "4-" and "piperidyl".
        (Some('-' | '\u{2010}'), Some(a))
            if previous.is_some_and(char::is_alphanumeric) && a.is_alphanumeric() =>
        {
            Join::Touch
        }
        (Some(b), Some(a)) if is_cjk(b) || is_cjk(a) => Join::Touch,
        _ => Join::Space,
    }
}

/// Han, kana and full-width forms: scripts written without spaces between
/// words. Hangul uses spaces and is not included.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{30FF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FF00}'..='\u{FFEF}'
        | '\u{20000}'..='\u{2FA1F}')
}

/// The list marker at the start of `text`: a bullet, or a number, letter
/// or roman numeral followed by `.` or `)`, or a number in brackets.
fn list_marker(text: &str) -> Option<String> {
    const BULLETS: &[char] = &[
        '•', '◦', '▪', '▫', '■', '□', '●', '○', '‣', '⁃', '–', '—', '-', '*', '✓', '✔', '➢', '►',
        '▶', '·',
    ];
    let (head, rest) = text.split_once(' ')?;
    if rest.trim().is_empty() {
        return None;
    }
    let mut chars = head.chars();
    let first = chars.next()?;
    if chars.as_str().is_empty() && BULLETS.contains(&first) {
        return Some(head.to_string());
    }
    let inner = head
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .filter(|h| h.chars().all(|c| c.is_ascii_digit()))
        .or_else(|| {
            let h = head.strip_prefix('(').unwrap_or(head);
            h.strip_suffix(['.', ')'])
        })?;
    let numbered =
        !inner.is_empty() && inner.len() <= 3 && inner.chars().all(|c| c.is_ascii_digit());
    let lettered = inner.len() == 1 && inner.chars().all(|c| c.is_ascii_alphabetic());
    let roman =
        !inner.is_empty() && inner.len() <= 5 && inner.chars().all(|c| "ivxlIVXL".contains(c));
    (numbered || lettered || roman).then(|| head.to_string())
}

/// Lay out the marks of one page. `top` and `bottom` are the page edges.
pub(crate) fn page(marks: Vec<Mark>, links: Vec<String>, top: f64, bottom: f64) -> Page {
    enum Item {
        Line(Line),
        Image(PartId),
    }
    let mut items = Vec::new();
    let mut line: Option<Line> = None;
    let mut space = false;
    for mark in marks {
        let glyph = match mark {
            Mark::Image(id) => {
                items.extend(line.take().map(Item::Line));
                items.push(Item::Image(id));
                continue;
            }
            Mark::Glyph(g) => g,
        };
        if glyph.text.chars().all(char::is_whitespace) {
            space = true;
            continue;
        }
        let rest = match line.as_mut() {
            Some(l) => l.extend(glyph, space),
            None => Some(glyph),
        };
        if let Some(glyph) = rest {
            items.extend(line.take().map(Item::Line));
            line = Some(Line::new(glyph));
        }
        space = false;
    }
    items.extend(line.take().map(Item::Line));

    let spacing = usual_spacing(items.iter().filter_map(|i| match i {
        Item::Line(l) => Some(l),
        Item::Image(_) => None,
    }));
    let mut sizes: BTreeMap<u64, usize> = BTreeMap::new();
    for item in &items {
        if let Item::Line(line) = item {
            for g in &line.glyphs {
                *sizes.entry(half_points(g.size)).or_default() += g.text.chars().count();
            }
        }
    }
    let mut pieces = Vec::new();
    let mut paragraph: Option<Paragraph> = None;
    for item in items {
        match item {
            Item::Image(id) => {
                pieces.extend(paragraph.take().map(|p| Piece::Text(p.freeze(&links))));
                pieces.push(Piece::Image(id));
            }
            Item::Line(line) => match paragraph.as_mut() {
                Some(p) if p.continues(&line, spacing) => p.push(line),
                _ => {
                    pieces.extend(paragraph.take().map(|p| Piece::Text(p.freeze(&links))));
                    paragraph = Some(Paragraph::new(line));
                }
            },
        }
    }
    pieces.extend(paragraph.map(|p| Piece::Text(p.freeze(&links))));
    Page {
        pieces,
        sizes,
        top,
        bottom,
    }
}

/// The usual distance between consecutive lines of the same size, in ems.
fn usual_spacing<'a>(lines: impl Iterator<Item = &'a Line>) -> f64 {
    let mut counts: BTreeMap<u64, usize> = BTreeMap::new();
    let mut prev: Option<&Line> = None;
    for line in lines {
        if let Some(p) = prev {
            let size = line.size();
            let ratio = (p.y - line.y) / size;
            if p.turn == line.turn
                && (p.size() - size).abs() <= 0.15 * size
                && (0.9..=3.0).contains(&ratio)
            {
                *counts.entry((ratio * 20.0).round() as u64).or_default() += 1;
            }
        }
        prev = Some(line);
    }
    counts
        .iter()
        .max_by_key(|(ratio, n)| (**n, std::cmp::Reverse(**ratio)))
        .map_or(1.2, |(ratio, _)| *ratio as f64 / 20.0)
}

/// The blocks of every page, and the running headers and footers taken
/// out of them.
pub(crate) fn blocks(pages: Vec<Page>) -> (Vec<Vec<Block>>, Vec<String>) {
    let body = body_size(&pages);
    let running = running(&pages);
    let headings: BTreeSet<u64> = texts(&pages)
        .filter(|t| is_heading(t, body))
        .map(|t| half_points(t.size))
        .collect();
    let level = |t: &Text| {
        let rank = headings
            .iter()
            .rev()
            .position(|s| *s == half_points(t.size))
            .unwrap_or(0);
        (rank + 1).min(6) as u8
    };

    let mut repeated: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::with_capacity(pages.len());
    for page in pages {
        let mut blocks = Vec::new();
        let mut list_indents: Vec<f64> = Vec::new();
        let (top, bottom) = (page.top, page.bottom);
        for piece in page.pieces {
            let mut t = match piece {
                Piece::Image(id) => {
                    blocks.push(Block::Image(ImageRef {
                        part: id,
                        alt: None,
                    }));
                    continue;
                }
                Piece::Text(t) => t,
            };
            if let Some(key) = running_key(top, bottom, &t)
                && running.contains(&key)
            {
                if seen.insert(key) {
                    repeated.push(plain_text(&t.content));
                }
                continue;
            }
            if is_heading(&t, body) {
                let text = plain_text(&t.content);
                blocks.push(Block::heading(level(&t), vec![Inline::text(text)]));
                list_indents.clear();
                continue;
            }
            let Some(marker) = t.marker.take() else {
                list_indents.clear();
                blocks.push(Block::paragraph(t.content));
                continue;
            };
            strip_marker(&mut t.content, &marker);
            while list_indents.last().is_some_and(|x| t.x0 < x - 0.5 * t.size) {
                list_indents.pop();
            }
            if list_indents.last().is_none_or(|x| t.x0 > x + 0.5 * t.size) {
                list_indents.push(t.x0);
            }
            blocks.push(Block::ListItem(ListItem {
                level: (list_indents.len().saturating_sub(1)).min(5) as u8,
                label: Some(marker),
                content: t.content,
            }));
        }
        out.push(blocks);
    }
    (out, repeated)
}

fn texts(pages: &[Page]) -> impl Iterator<Item = &Text> {
    pages.iter().flat_map(|page| {
        page.pieces.iter().filter_map(|piece| match piece {
            Piece::Text(t) => Some(t),
            Piece::Image(_) => None,
        })
    })
}

/// The size most of the document's text is set in.
fn body_size(pages: &[Page]) -> f64 {
    let mut counts: BTreeMap<u64, usize> = BTreeMap::new();
    for (size, n) in pages.iter().flat_map(|p| &p.sizes) {
        *counts.entry(*size).or_default() += n;
    }
    counts
        .iter()
        .max_by_key(|(size, n)| (**n, std::cmp::Reverse(**size)))
        .map_or(0.0, |(size, _)| *size as f64 / 2.0)
}

fn is_heading(t: &Text, body: f64) -> bool {
    body > 0.0
        && t.size >= body * HEADING_SCALE
        && t.lines <= HEADING_LINES
        && t.chars <= HEADING_CHARS
        && t.alphabetic
}

/// The key that identifies a paragraph as a running header or footer: its
/// text with digits replaced, when it sits at the top or bottom of the
/// page.
fn running_key(top: f64, bottom: f64, t: &Text) -> Option<String> {
    let band = (top - bottom) * RUNNING_BAND;
    let edge = t.top >= top - band || t.bottom <= bottom + band;
    if !edge || t.turn != 0 || t.lines > 2 || band <= 0.0 {
        return None;
    }
    let text: String = plain_text(&t.content)
        .chars()
        .map(|c| if c.is_numeric() { '#' } else { c })
        .collect();
    let key = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!key.is_empty()).then_some(key)
}

/// Keys of the paragraphs repeated at the top or bottom of most pages.
fn running(pages: &[Page]) -> BTreeSet<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut with_text = 0;
    for page in pages {
        let keys: BTreeSet<String> = page
            .pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Text(t) => running_key(page.top, page.bottom, t),
                Piece::Image(_) => None,
            })
            .collect();
        if page.pieces.iter().any(|p| matches!(p, Piece::Text(_))) {
            with_text += 1;
        }
        for key in keys {
            *counts.entry(key).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, n)| *n >= RUNNING_PAGES && *n * 2 >= with_text)
        .map(|(key, _)| key)
        .collect()
}

/// Remove the list marker from the start of a list item's text.
fn strip_marker(content: &mut Vec<Inline>, marker: &str) {
    if let Some(Inline::Text(first)) = content.first_mut()
        && let Some(rest) = first.text.strip_prefix(marker)
    {
        first.text = rest.trim_start().to_string();
        if first.text.is_empty() {
            content.remove(0);
            if let Some(Inline::Text(next)) = content.first_mut() {
                next.text = next.text.trim_start().to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doctopus_core::blocks_plain_text;

    /// Glyphs for `text` from (x, y) at `size`, each `0.5 * size` wide.
    fn word(marks: &mut Vec<Mark>, text: &str, x: f64, y: f64, size: f64) {
        for (i, c) in text.chars().enumerate() {
            marks.push(Mark::Glyph(Glyph {
                x: x + i as f64 * size * 0.5,
                y,
                size,
                width: size * 0.5,
                turn: 0,
                text: c.to_string(),
                style: Style::default(),
                link: None,
            }));
        }
    }

    /// A line of words, set from `x` with a quarter-em space between them.
    fn line(marks: &mut Vec<Mark>, text: &str, x: f64, y: f64, size: f64) {
        let mut at = x;
        for w in text.split(' ') {
            word(marks, w, at, y, size);
            at += (w.chars().count() as f64 * 0.5 + 0.25) * size;
        }
    }

    fn render(pages: Vec<Page>) -> Vec<String> {
        let (blocks, _) = blocks(pages);
        blocks.iter().map(|b| blocks_plain_text(b)).collect()
    }

    fn markdownish(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .map(|b| match b {
                Block::Heading(h) => format!("h{} {}", h.level, plain_text(&h.content)),
                Block::Paragraph(p) => format!("p {}", plain_text(&p.content)),
                Block::ListItem(l) => format!(
                    "li{} {} {}",
                    l.level,
                    l.label.as_deref().unwrap_or(""),
                    plain_text(&l.content)
                ),
                Block::Image(i) => format!("img {}", i.part.as_str()),
                _ => "?".into(),
            })
            .collect()
    }

    #[test]
    fn words_lines_and_paragraphs_are_found() {
        let mut marks = Vec::new();
        line(&mut marks, "The first line of", 72.0, 700.0, 10.0);
        line(&mut marks, "a paragraph.", 72.0, 688.0, 10.0);
        line(&mut marks, "Another para-", 72.0, 664.0, 10.0);
        line(&mut marks, "graph follows.", 72.0, 652.0, 10.0);
        let page = page(marks, Vec::new(), 792.0, 0.0);
        assert_eq!(
            render(vec![page]),
            ["The first line of a paragraph.\nAnother paragraph follows."]
        );
    }

    #[test]
    fn centered_lines_and_broken_compounds_join() {
        let mut marks = Vec::new();
        line(&mut marks, "A Title Set Over", 72.0, 740.0, 20.0);
        line(&mut marks, "Two Lines", 107.0, 716.0, 20.0);
        line(&mut marks, "Stress-", 72.0, 700.0, 10.0);
        line(&mut marks, "Engineered and well-", 72.0, 688.0, 10.0);
        line(&mut marks, "known parts, 4-", 72.0, 676.0, 10.0);
        line(&mut marks, "piperidyl and a -", 72.0, 664.0, 10.0);
        line(&mut marks, "dash.", 72.0, 652.0, 10.0);
        let page = page(marks, Vec::new(), 792.0, 0.0);
        let (blocks, _) = blocks(vec![page]);
        assert_eq!(
            markdownish(&blocks[0]),
            [
                "h1 A Title Set Over Two Lines",
                "p Stress-Engineered and wellknown parts, 4-piperidyl and a - dash.",
            ]
        );
    }

    #[test]
    fn indents_markers_and_size_changes_start_paragraphs() {
        let mut marks = Vec::new();
        line(&mut marks, "Big Title", 72.0, 740.0, 20.0);
        line(&mut marks, "Body text runs on and on", 72.0, 700.0, 10.0);
        line(&mut marks, "over two lines.", 72.0, 688.0, 10.0);
        line(&mut marks, "An indented start.", 90.0, 676.0, 10.0);
        line(&mut marks, "• first item", 72.0, 664.0, 10.0);
        line(&mut marks, "continued", 80.0, 652.0, 10.0);
        line(&mut marks, "• second item", 72.0, 640.0, 10.0);
        line(&mut marks, "1. numbered", 82.0, 628.0, 10.0);
        let page = page(marks, Vec::new(), 792.0, 0.0);
        let (blocks, _) = blocks(vec![page]);
        assert_eq!(
            markdownish(&blocks[0]),
            [
                "h1 Big Title",
                "p Body text runs on and on over two lines.",
                "p An indented start.",
                "li0 • first item continued",
                "li0 • second item",
                "li1 1. numbered",
            ]
        );
    }

    #[test]
    fn columns_and_images_split_paragraphs() {
        let mut marks = Vec::new();
        line(&mut marks, "Left column", 72.0, 700.0, 10.0);
        line(&mut marks, "text.", 72.0, 688.0, 10.0);
        marks.push(Mark::Image(PartId::new("img1")));
        line(&mut marks, "Right column", 320.0, 700.0, 10.0);
        line(&mut marks, "text.", 320.0, 688.0, 10.0);
        let page = page(marks, Vec::new(), 792.0, 0.0);
        let (blocks, _) = blocks(vec![page]);
        assert_eq!(
            markdownish(&blocks[0]),
            ["p Left column text.", "img img1", "p Right column text."]
        );
    }

    #[test]
    fn overprinted_glyphs_are_read_once() {
        let mut marks = Vec::new();
        word(&mut marks, "Bold", 72.0, 700.0, 10.0);
        word(&mut marks, "Bold", 72.3, 700.0, 10.0);
        line(&mut marks, "text", 96.0, 700.0, 10.0);
        let page = page(marks, Vec::new(), 792.0, 0.0);
        assert_eq!(render(vec![page]), ["Bold text"]);
    }

    #[test]
    fn cjk_lines_join_without_spaces() {
        let mut marks = Vec::new();
        word(&mut marks, "日本語の", 72.0, 700.0, 10.0);
        word(&mut marks, "文章です", 72.0, 688.0, 10.0);
        let page = page(marks, Vec::new(), 792.0, 0.0);
        assert_eq!(render(vec![page]), ["日本語の文章です"]);
    }

    #[test]
    fn links_cover_their_glyphs() {
        let mut marks = Vec::new();
        line(&mut marks, "see the site now", 72.0, 700.0, 10.0);
        for m in &mut marks {
            if let Mark::Glyph(g) = m
                && (89.0..128.0).contains(&g.x)
            {
                g.link = Some(0);
            }
        }
        let page = page(marks, vec!["https://example.com".into()], 792.0, 0.0);
        let (blocks, _) = blocks(vec![page]);
        let Block::Paragraph(p) = &blocks[0][0] else {
            panic!("{:?}", blocks[0]);
        };
        let link = p.content.iter().find_map(|i| match i {
            Inline::Link(l) => Some(l),
            _ => None,
        });
        assert_eq!(
            link.map(|l| plain_text(&l.content)).as_deref(),
            Some("the site")
        );
        assert_eq!(plain_text(&p.content), "see the site now");
    }

    #[test]
    fn running_headers_and_page_numbers_are_taken_out() {
        let pages: Vec<Page> = (1..=5)
            .map(|n| {
                let mut marks = Vec::new();
                line(&mut marks, "Journal of Tests", 72.0, 760.0, 9.0);
                line(&mut marks, &format!("Body of page {n}."), 72.0, 700.0, 10.0);
                line(&mut marks, &n.to_string(), 300.0, 30.0, 9.0);
                page(marks, Vec::new(), 792.0, 0.0)
            })
            .collect();
        let (blocks, repeated) = blocks(pages);
        assert_eq!(repeated, ["Journal of Tests", "1"]);
        assert_eq!(blocks_plain_text(&blocks[2]), "Body of page 3.");
    }

    #[test]
    fn list_markers_are_recognized() {
        for text in ["• a", "1. a", "12) a", "(b) a", "iv. a", "[3] a", "- a"] {
            assert!(list_marker(text).is_some(), "{text}");
        }
        for text in ["2019. a", "word a", "•", "1.5 a", "Fig. a"] {
            assert!(list_marker(text).is_none(), "{text}");
        }
    }
}
