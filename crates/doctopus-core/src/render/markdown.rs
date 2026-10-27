//! Markdown and plain-text rendering.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

use super::{RenderOptions, Rendered, SectionMark, human_size, shell_quote, yaml};
use crate::{
    Address, Block, Chart, Diagram, Document, Grid, Group, GroupKind, Inline, ListItem, Note,
    NoteKind, Part, PartId, Section, SectionKind, Shape, Style, Table, plain_text,
};

/// Render a document as Markdown.
pub fn markdown(doc: &Document, options: &RenderOptions) -> Rendered {
    Writer::new(Flavor::Markdown, options).finish(doc)
}

/// Render a document as plain text: the same content and layout as
/// [`markdown`] without emphasis, link or table syntax.
pub fn text(doc: &Document, options: &RenderOptions) -> Rendered {
    Writer::new(Flavor::Text, options).finish(doc)
}

/// How many parts the YAML header lists before pointing at `info`.
const FRONT_MATTER_PARTS: usize = 25;
/// Grids whose table would have more positions than this, most of them
/// empty, are listed cell by cell instead.
const SPARSE_GRID_AREA: u64 = 1_000;
/// Diagrams with more connections than this get no Mermaid flowchart in
/// standalone output: drawn, it would be unreadable.
const FLOWCHART_CONNECTIONS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flavor {
    Markdown,
    Text,
}

/// Position of the document being rendered in the part tree.
#[derive(Debug, Clone)]
struct Scope {
    /// Address of this document (`deck.pptx#emb2`).
    address: Address,
    /// Part ids from the rendered root down to this document.
    path: Vec<String>,
    /// Nesting depth below the rendered root.
    depth: u32,
}

impl Scope {
    fn part_path(&self, id: &PartId) -> String {
        let mut path = self.path.clone();
        path.push(id.to_string());
        path.join("/")
    }

    /// The key of an anchor named `name` in this document.
    fn anchor(&self, name: &str) -> String {
        format!("{}#{name}", self.path.join("/"))
    }

    fn child(&self, id: &PartId) -> Self {
        let mut path = self.path.clone();
        path.push(id.to_string());
        Self {
            address: self.address.child(id.as_str()),
            path,
            depth: self.depth + 1,
        }
    }
}

struct Writer<'o> {
    out: String,
    flavor: Flavor,
    options: &'o RenderOptions,
    marks: Vec<(usize, SectionMark)>,
    /// Part paths already referenced, so unplaced parts can be listed and
    /// embedded documents used twice are only inlined once.
    seen: BTreeSet<String>,
    /// The list items just written, innermost last: their level and the
    /// column their text starts at, where nested items must start.
    list: Vec<(u8, usize)>,
    /// How often each heading id was handed out, to number repeats the
    /// way GitHub does (`notes`, `notes-1`).
    slugs: HashMap<String, usize>,
    /// Heading ids by [`Scope::anchor`] key, as found while writing.
    anchors: HashMap<String, String>,
    /// Heading ids by anchor key, from a first pass over the whole
    /// document, so links can point forward.
    resolved: HashMap<String, String>,
}

impl<'o> Writer<'o> {
    fn new(flavor: Flavor, options: &'o RenderOptions) -> Self {
        Self {
            out: String::new(),
            flavor,
            options,
            marks: Vec::new(),
            seen: BTreeSet::new(),
            list: Vec::new(),
            slugs: HashMap::new(),
            anchors: HashMap::new(),
            resolved: HashMap::new(),
        }
    }

    /// A writer for content that is written apart and then quoted.
    fn nested(&self) -> Self {
        let mut sub = Self::new(self.flavor, self.options);
        sub.slugs.clone_from(&self.slugs);
        sub.resolved.clone_from(&self.resolved);
        sub
    }

    /// Take over what a [`nested`](Self::nested) writer learned.
    fn absorb(&mut self, sub: &mut Self) {
        self.seen.append(&mut sub.seen);
        self.slugs = std::mem::take(&mut sub.slugs);
        self.anchors.extend(sub.anchors.drain());
    }

    fn finish(mut self, doc: &Document) -> Rendered {
        let scope = Scope {
            address: self.options.source.clone(),
            path: Vec::new(),
            depth: 0,
        };
        // Links to headings need the ids of headings further down: find
        // them all first.
        if self.md() && has_internal_links(doc) {
            let mut first = Self::new(self.flavor, self.options);
            first.document(doc, &scope);
            self.resolved = first.anchors;
        }
        if self.options.front_matter {
            self.front_matter(doc);
        }
        self.document(doc, &scope);
        let text = self.out.trim_end().to_string() + "\n";
        let marks = to_char_offsets(&text, self.marks);
        Rendered { text, marks }
    }

    // ----- layout helpers -------------------------------------------------

    fn blank_line(&mut self) {
        if self.out.is_empty() {
            return;
        }
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        if !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    fn line_break(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn md(&self) -> bool {
        self.flavor == Flavor::Markdown
    }

    // ----- document structure ---------------------------------------------

    fn front_matter(&mut self, doc: &Document) {
        let mut fm = String::from("---\n");
        let mut kv = |k: &str, v: &str| {
            let _ = writeln!(fm, "{}: {}", yaml::key(k), yaml::scalar(v));
        };
        kv("source", &self.options.source.to_string());
        kv(
            "format",
            &format!("{} ({})", doc.format.id, doc.format.name),
        );
        for (k, v) in doc.metadata.fields() {
            kv(k, &one_line(v, 200));
        }
        for (noun, count) in section_counts(doc) {
            let _ = writeln!(fm, "{noun}: {count}");
        }
        if !doc.parts.is_empty() {
            fm.push_str("parts:\n");
            for part in doc.parts.iter().take(FRONT_MATTER_PARTS) {
                let _ = writeln!(
                    fm,
                    "  - {}: {}",
                    part.id,
                    yaml::scalar(&describe_part(part))
                );
            }
            if doc.parts.len() > FRONT_MATTER_PARTS {
                let rest = doc.parts.len() - FRONT_MATTER_PARTS;
                let more = if self.options.standalone {
                    format!("... {rest} more, linked where they appear")
                } else {
                    format!(
                        "... {rest} more; list them all with: doctopus info {}",
                        shell_quote(&self.options.source.to_string())
                    )
                };
                let _ = writeln!(fm, "  - {}", yaml::scalar(&more));
            }
        }
        if !doc.warnings.is_empty() {
            fm.push_str("warnings:\n");
            for w in &doc.warnings {
                let _ = writeln!(fm, "  - {}", yaml::scalar(&one_line(w, 300)));
            }
        }
        fm.push_str("---\n\n");
        self.out.push_str(&fm);
    }

    fn document(&mut self, doc: &Document, scope: &Scope) {
        for section in &doc.sections {
            self.section(doc, section, scope);
        }
        self.unplaced_parts(doc, scope);
    }

    fn section(&mut self, doc: &Document, section: &Section, scope: &Scope) {
        let level = heading_level(2, scope.depth);
        if let Some(title) = section_title(section) {
            self.blank_line();
            if scope.depth == 0 {
                self.marks.push((
                    self.out.len(),
                    SectionMark {
                        kind: section.kind,
                        number: section.number,
                        name: section.name.clone(),
                        offset: 0,
                    },
                ));
            }
            let slug = self.titled_heading(level, &title, &title);
            // Readers link to sheets, slides and pages as `#slide-3`.
            if let (true, Some(n)) = (section.kind.is_primary(), section.number) {
                let name = format!("{}-{n}", section.kind.noun());
                self.anchors.insert(scope.anchor(&name), slug);
            }
            if !section.properties.is_empty() {
                let props: Vec<String> = section
                    .properties
                    .iter()
                    .map(|(k, v)| format!("{}: {v}", k.replace('_', " ")))
                    .collect();
                self.blank_line();
                if self.md() {
                    let _ = writeln!(self.out, "*{}*", props.join(" · "));
                } else {
                    let _ = writeln!(self.out, "({})", props.join(" · "));
                }
            }
        }
        // Headings inside a sheet or slide sit below the section heading;
        // the body's own headings keep their levels.
        let base = if section.kind == SectionKind::Body {
            scope.depth
        } else {
            u32::from(level)
        };
        self.blocks(doc, &section.blocks, scope, base);
        if section.blocks.iter().all(Block::is_blank) && section.kind.is_primary() {
            self.blank_line();
            self.out.push_str(if self.md() {
                "*(empty)*\n"
            } else {
                "(empty)\n"
            });
        }
    }

    fn heading(&mut self, level: u8, title: &str) {
        self.titled_heading(level, title, title);
    }

    /// Write a heading whose text without Markdown syntax is `plain`, and
    /// return the id viewers give it, for links.
    fn titled_heading(&mut self, level: u8, title: &str, plain: &str) -> String {
        self.blank_line();
        if !self.md() {
            let _ = writeln!(self.out, "{}", one_line(title, 300));
            return String::new();
        }
        let _ = writeln!(
            self.out,
            "{} {}",
            "#".repeat(usize::from(level)),
            one_line(title, 300)
        );
        self.slug(&one_line(plain, 300))
    }

    /// The id GitHub gives a heading with this text: lower case, spaces
    /// as `-`, punctuation dropped, and `-1`, `-2`... for repeats.
    fn slug(&mut self, text: &str) -> String {
        let base: String = text
            .to_lowercase()
            .chars()
            .filter_map(|c| match c {
                ' ' => Some('-'),
                '-' | '_' => Some(c),
                c if c.is_alphanumeric() => Some(c),
                _ => None,
            })
            .collect();
        let mut slug = base.clone();
        while self.slugs.contains_key(&slug) {
            let n = self.slugs.entry(base.clone()).or_default();
            *n += 1;
            slug = format!("{base}-{n}");
        }
        self.slugs.insert(slug.clone(), 0);
        slug
    }

    fn blocks(&mut self, doc: &Document, blocks: &[Block], scope: &Scope, heading_base: u32) {
        let mut in_list = false;
        for block in blocks {
            if block.is_blank() {
                continue;
            }
            let is_list = matches!(block, Block::ListItem(_));
            if is_list && in_list {
                self.line_break();
            } else {
                self.blank_line();
            }
            if !is_list {
                self.list.clear();
            }
            in_list = is_list;
            self.block(doc, block, scope, heading_base);
        }
    }

    fn block(&mut self, doc: &Document, block: &Block, scope: &Scope, heading_base: u32) {
        match block {
            Block::Heading(h) => {
                let text = self.inlines(doc, &h.content, scope, false);
                let level = heading_level(u32::from(h.level), heading_base);
                let slug = self.titled_heading(level, &text, &plain_text(&h.content));
                for anchor in &h.anchors {
                    self.anchors.insert(scope.anchor(anchor), slug.clone());
                }
            }
            Block::Paragraph(p) => {
                let text = self.inlines(doc, &p.content, scope, false);
                let text = hard_breaks(text.trim(), self.md());
                self.out.push_str(&escape_line_start(&text, self.md()));
                self.out.push('\n');
            }
            Block::ListItem(item) => self.list_item(doc, item, scope),
            Block::Table(table) => self.table(doc, table, scope),
            Block::Grid(grid) => self.grid(grid),
            Block::Image(image) => {
                let rendered = self.image(doc, &image.part, image.alt.as_deref(), scope);
                self.out.push_str(&rendered);
                self.out.push('\n');
            }
            Block::Embed(embed) => self.embed(doc, &embed.part, scope),
            Block::Chart(chart) => self.chart(chart),
            Block::Diagram(diagram) => self.diagram(doc, diagram, scope),
            Block::Math(math) => {
                if self.md() {
                    let _ = writeln!(self.out, "$$\n{}\n$$", math.tex.trim());
                } else {
                    let _ = writeln!(self.out, "{}", math.tex.trim());
                }
            }
            Block::Code(code) => {
                let text = code.text.trim_end_matches(['\n', '\r']);
                if self.md() {
                    let fence = fence(text);
                    let language = code.language.as_deref().unwrap_or_default();
                    let _ = writeln!(self.out, "{fence}{language}\n{text}\n{fence}");
                } else {
                    let _ = writeln!(self.out, "{text}");
                }
            }
            Block::Note(note) => self.note(doc, note, scope),
            Block::Group(group) => self.group(doc, group, scope, heading_base),
        }
    }

    fn list_item(&mut self, doc: &Document, item: &ListItem, scope: &Scope) {
        let text = self.inlines(doc, &item.content, scope, false);
        let label = item
            .label
            .as_deref()
            .map(str::trim)
            .filter(|l| !is_bullet(l));
        if !self.md() {
            let indent = "  ".repeat(usize::from(item.level));
            let text = text.trim().replace('\n', &format!("\n{indent}  "));
            let _ = writeln!(self.out, "{indent}{} {text}", label.unwrap_or("-"));
            return;
        }
        // A label Markdown has no list marker for (`a)`, `4.2`) stays in
        // the text of a bullet.
        let (marker, text) = match label {
            Some(label) if is_ordered_marker(label) => (label, text.trim().to_string()),
            Some(label) => ("-", format!("{label} {}", text.trim())),
            None => ("-", text.trim().to_string()),
        };
        // Nested items start where their parent's text starts, which for
        // `10.` is further in than for `-`.
        while self
            .list
            .last()
            .is_some_and(|&(level, _)| level >= item.level)
        {
            self.list.pop();
        }
        let indent = self.list.last().map_or(0, |&(_, column)| column);
        let column = indent + marker.chars().count() + 1;
        self.list.push((item.level, column));
        let text = hard_breaks(&text, true).replace('\n', &format!("\n{}", " ".repeat(column)));
        let _ = writeln!(self.out, "{}{marker} {text}", " ".repeat(indent));
    }

    // ----- tables -----------------------------------------------------------

    fn table(&mut self, doc: &Document, table: &Table, scope: &Scope) {
        let width = table
            .rows
            .iter()
            .map(|r| {
                r.cells
                    .iter()
                    .map(|c| c.col_span.max(1) as usize)
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0);
        if width == 0 {
            return;
        }
        let rows: Vec<Vec<String>> = table
            .rows
            .iter()
            .map(|row| {
                let mut cells = Vec::with_capacity(width);
                for cell in &row.cells {
                    let text = if cell.merged {
                        String::new()
                    } else {
                        self.cell_text(doc, &cell.blocks, scope)
                    };
                    cells.push(text);
                    for _ in 1..cell.col_span.max(1) {
                        cells.push(String::new());
                    }
                }
                cells.resize(width, String::new());
                cells
            })
            .collect();
        self.write_table(&rows, true);
    }

    /// Flatten cell content to a single line: paragraphs are joined with
    /// `<br>`, pictures and objects become references.
    fn cell_text(&mut self, doc: &Document, blocks: &[Block], scope: &Scope) -> String {
        let mut pieces = Vec::new();
        for block in blocks {
            let piece = match block {
                Block::Heading(h) => self.inlines(doc, &h.content, scope, true),
                Block::Paragraph(p) => self.inlines(doc, &p.content, scope, true),
                Block::ListItem(l) => {
                    let marker = l.label.as_deref().filter(|m| !is_bullet(m)).unwrap_or("•");
                    format!("{marker} {}", self.inlines(doc, &l.content, scope, true))
                }
                Block::Table(t) => t
                    .rows
                    .iter()
                    .map(|r| {
                        r.cells
                            .iter()
                            .map(|c| self.cell_text(doc, &c.blocks, scope))
                            .collect::<Vec<_>>()
                            .join(" / ")
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
                Block::Image(img) => self.image(doc, &img.part, img.alt.as_deref(), scope),
                // Rendered in full after the content (see `unplaced_parts`).
                Block::Embed(e) => format!("[embedded {}]", scope.address.child(e.part.as_str())),
                Block::Math(m) => format!("${}$", m.tex),
                Block::Code(c) if self.md() => c
                    .text
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(code_span)
                    .collect::<Vec<_>>()
                    .join("<br>"),
                Block::Code(c) => c.text.replace('\n', " "),
                Block::Group(g) => self.cell_text(doc, &g.blocks, scope),
                Block::Note(n) => self.cell_text(doc, &n.blocks, scope),
                Block::Grid(_) | Block::Chart(_) | Block::Diagram(_) => String::new(),
            };
            let piece = piece.trim().to_string();
            if !piece.is_empty() {
                pieces.push(piece);
            }
        }
        let sep = if self.md() { "<br>" } else { " / " };
        pieces.join(sep).replace('\n', sep)
    }

    fn write_table(&mut self, rows: &[Vec<String>], header: bool) {
        if rows.is_empty() {
            return;
        }
        if !self.md() {
            for row in rows {
                let _ = writeln!(self.out, "{}", row.join("\t").trim_end());
            }
            return;
        }
        let esc = |s: &str| s.replace('|', "\\|");
        let write_row = |out: &mut String, row: &[String]| {
            out.push('|');
            for cell in row {
                let _ = write!(out, " {} |", esc(cell.trim()));
            }
            out.push('\n');
        };
        let width = rows[0].len();
        if header {
            write_row(&mut self.out, &rows[0]);
            let _ = writeln!(self.out, "|{}", "---|".repeat(width));
            for row in &rows[1..] {
                write_row(&mut self.out, row);
            }
        } else {
            write_row(&mut self.out, &vec![String::new(); width]);
            let _ = writeln!(self.out, "|{}", "---|".repeat(width));
            for row in rows {
                write_row(&mut self.out, row);
            }
        }
    }

    fn grid(&mut self, grid: &Grid) {
        let Some((min_col, max_col)) = grid
            .rows
            .iter()
            .flat_map(|r| r.cells.iter().map(|c| c.col))
            .fold(None, |acc: Option<(u32, u32)>, c| {
                Some(acc.map_or((c, c), |(lo, hi)| (lo.min(c), hi.max(c))))
            })
        else {
            self.out.push_str(if self.md() {
                "*(no cells)*\n"
            } else {
                "(no cells)\n"
            });
            return;
        };
        let cells: usize = grid.rows.iter().map(|r| r.cells.len()).sum();
        let area = grid.rows.len() as u64 * u64::from(max_col - min_col + 1);
        let sparse = area > SPARSE_GRID_AREA && (cells as u64) * 5 < area;
        let mut rows = Vec::with_capacity(grid.rows.len() + 1);
        if sparse {
            let _ = writeln!(self.out, "Cells (sparse, listed by address):");
        } else {
            let mut header = vec![String::new()];
            header.extend((min_col..=max_col).map(crate::CellRef::column_name));
            rows.push(header);
        }
        let mut notes = Vec::new();
        let mut links = Vec::new();
        for row in &grid.rows {
            let mut line = vec![(row.row + 1).to_string()];
            if !sparse {
                line.resize((max_col - min_col) as usize + 2, String::new());
            }
            for cell in &row.cells {
                let mut value = cell.value.display();
                if self.options.formulas
                    && let Some(f) = &cell.formula
                {
                    value = if value.is_empty() {
                        format!("={f}")
                    } else {
                        format!("{value} (={f})")
                    };
                }
                let address = crate::CellRef::new(row.row, cell.col);
                let sep = if self.md() { "<br>" } else { " " };
                let value = no_links(value.trim(), self.md())
                    .replace("\r\n", "\n")
                    .replace('\n', sep);
                if sparse {
                    let _ = writeln!(self.out, "- {address}: {value}");
                } else {
                    line[(cell.col - min_col) as usize + 1] = value;
                }
                if let Some(c) = &cell.comment {
                    notes.push(format!("{address}: {}", one_line(c, 500)));
                }
                if let Some(l) = &cell.link {
                    links.push(format!("{address}: {l}"));
                }
            }
            if !sparse {
                rows.push(line);
            }
        }
        if !sparse {
            self.write_table(&rows, true);
        }
        if !grid.merged.is_empty() {
            let merged: Vec<String> = grid.merged.iter().map(ToString::to_string).collect();
            self.blank_line();
            let _ = writeln!(self.out, "Merged cells: {}", merged.join(", "));
        }
        self.labelled_list("Cell notes", &notes);
        self.labelled_list("Cell links", &links);
    }

    fn labelled_list(&mut self, label: &str, items: &[String]) {
        if items.is_empty() {
            return;
        }
        self.blank_line();
        let _ = writeln!(self.out, "{label}:");
        for item in items {
            let _ = writeln!(self.out, "- {item}");
        }
    }

    fn chart(&mut self, chart: &Chart) {
        let title = chart.title.as_deref().map(|t| one_line(t, 200));
        let heading = match &title {
            Some(t) => format!("Chart: {t} ({})", chart.kind),
            None => format!("Chart ({})", chart.kind),
        };
        if self.md() {
            let _ = writeln!(self.out, "**{heading}**");
        } else {
            let _ = writeln!(self.out, "{heading}");
        }
        if chart.series.is_empty() {
            return;
        }
        let points = chart
            .series
            .iter()
            .map(|s| s.values.len())
            .max()
            .unwrap_or(0)
            .max(chart.categories.len());
        let mut rows = Vec::with_capacity(points + 1);
        let mut header = vec![String::new()];
        for (i, s) in chart.series.iter().enumerate() {
            header.push(
                s.name
                    .clone()
                    .unwrap_or_else(|| format!("Series {}", i + 1)),
            );
        }
        rows.push(header);
        for p in 0..points {
            let mut row = vec![
                chart
                    .categories
                    .get(p)
                    .cloned()
                    .unwrap_or_else(|| (p + 1).to_string()),
            ];
            for s in &chart.series {
                row.push(
                    s.values
                        .get(p)
                        .copied()
                        .flatten()
                        .map(crate::format_number)
                        .unwrap_or_default(),
                );
            }
            rows.push(row);
        }
        self.blank_line();
        self.write_table(&rows, true);
    }

    fn diagram(&mut self, doc: &Document, diagram: &Diagram, scope: &Scope) {
        let mut lines = Vec::new();
        let mut parts = Vec::new();
        let connected: HashSet<&str> = diagram
            .connections
            .iter()
            .flat_map(|c| [c.from.as_str(), c.to.as_str()])
            .collect();
        for shape in &diagram.shapes {
            self.shape_lines(shape, 0, scope, &connected, &mut lines, &mut parts);
        }
        if !lines.is_empty() {
            let _ = writeln!(self.out, "Shapes:");
            for line in &lines {
                let _ = writeln!(self.out, "{line}");
            }
        } else if diagram.connections.is_empty() {
            fn count(shapes: &[Shape]) -> usize {
                shapes.iter().map(|s| 1 + count(&s.children)).sum()
            }
            let n = count(&diagram.shapes);
            let note = format!("{n} shape{} without text", if n == 1 { "" } else { "s" });
            let _ = writeln!(
                self.out,
                "{}",
                if self.md() {
                    format!("*({note})*")
                } else {
                    format!("({note})")
                }
            );
        }
        if !diagram.connections.is_empty() {
            let names = shape_names(&diagram.shapes);
            self.blank_line();
            let _ = writeln!(self.out, "Connections:");
            for c in &diagram.connections {
                let name = |id: &str| {
                    names
                        .iter()
                        .find(|(i, _)| i == id)
                        .map(|(_, n)| format!("{id} {n}"))
                        .unwrap_or_else(|| id.to_string())
                };
                let label = c
                    .label
                    .as_deref()
                    .map(|l| format!(" ({})", one_line(l, 120)))
                    .unwrap_or_default();
                let via = c
                    .connector
                    .as_deref()
                    .map(|id| format!(" via {id}"))
                    .unwrap_or_default();
                let inferred = if c.inferred {
                    " [inferred from positions]"
                } else {
                    ""
                };
                let _ = writeln!(
                    self.out,
                    "- {} -> {}{label}{via}{inferred}",
                    name(&c.from),
                    name(&c.to)
                );
            }
            if self.options.standalone
                && self.md()
                && diagram.connections.len() <= FLOWCHART_CONNECTIONS
            {
                self.flowchart(diagram);
            }
        }
        // Objects shown by shapes are rendered after the shape list; pictures
        // are fully described by the reference on the shape's line.
        for id in parts {
            if doc
                .part(id.as_str())
                .is_some_and(|p| p.kind == crate::PartKind::Image)
            {
                self.seen.insert(scope.part_path(&id));
            } else {
                self.blank_line();
                self.embed(doc, &id, scope);
            }
        }
    }

    /// The connections of a diagram as a Mermaid flowchart, which Markdown
    /// viewers draw. Shapes are numbered in order of first use; connections
    /// inferred from positions are dotted.
    fn flowchart(&mut self, diagram: &Diagram) {
        fn node<'d>(nodes: &mut Vec<&'d str>, id: &'d str) -> usize {
            match nodes.iter().position(|n| *n == id) {
                Some(i) => i + 1,
                None => {
                    nodes.push(id);
                    nodes.len()
                }
            }
        }
        let mut labels = Vec::new();
        shape_labels(&diagram.shapes, &mut labels);
        let mut nodes = Vec::new();
        let mut edges = String::new();
        for c in &diagram.connections {
            let from = node(&mut nodes, &c.from);
            let to = node(&mut nodes, &c.to);
            let arrow = if c.inferred { "-.->" } else { "-->" };
            let label = c
                .label
                .as_deref()
                .map(|l| format!("|\"{}\"|", mermaid_text(&one_line(l, 60))))
                .unwrap_or_default();
            let _ = writeln!(edges, "    n{from} {arrow}{label} n{to}");
        }
        self.blank_line();
        self.out.push_str("```mermaid\nflowchart TD\n");
        for (i, id) in nodes.iter().enumerate() {
            let label = labels
                .iter()
                .find(|(shape, _)| shape == id)
                .map_or_else(|| format!("shape {id}"), |(_, label)| label.clone());
            let _ = writeln!(self.out, "    n{}[\"{}\"]", i + 1, mermaid_text(&label));
        }
        self.out.push_str(&edges);
        self.out.push_str("```\n");
    }

    fn shape_lines(
        &self,
        shape: &Shape,
        depth: usize,
        scope: &Scope,
        connected: &HashSet<&str>,
        lines: &mut Vec<String>,
        parts: &mut Vec<PartId>,
    ) {
        let mut line = format!("{}- {}", "  ".repeat(depth), shape.id);
        if let Some(kind) = &shape.kind {
            let _ = write!(line, " [{kind}]");
        }
        if !shape.text.trim().is_empty() {
            let _ = write!(line, " \"{}\"", one_line(&shape.text, 300));
        }
        if !shape.data.is_empty() {
            let data: Vec<String> = shape
                .data
                .iter()
                .map(|(k, v)| format!("{k}: {}", one_line(v, 120)))
                .collect();
            let _ = write!(line, " ({})", data.join("; "));
        }
        for link in &shape.links {
            let _ = write!(line, " <{link}>");
        }
        if let Some(part) = &shape.part {
            let _ = write!(line, " -> {part} ({})", self.reference(scope, part));
            parts.push(part.clone());
        }
        let mut child_lines = Vec::new();
        for child in &shape.children {
            self.shape_lines(child, depth + 1, scope, connected, &mut child_lines, parts);
        }
        // Groups are listed when they or any member say something.
        let has_content = shape.kind.is_some()
            || !shape.text.trim().is_empty()
            || !shape.data.is_empty()
            || !shape.links.is_empty()
            || shape.part.is_some()
            || !child_lines.is_empty()
            || connected.contains(shape.id.as_str());
        if has_content {
            lines.push(line);
            lines.append(&mut child_lines);
        }
    }

    // ----- notes and groups -----------------------------------------------

    fn note(&mut self, doc: &Document, note: &Note, scope: &Scope) {
        let mut body = Vec::new();
        for block in &note.blocks {
            let text = match block {
                Block::Paragraph(p) => self.inlines(doc, &p.content, scope, false),
                Block::ListItem(l) => format!("- {}", self.inlines(doc, &l.content, scope, false)),
                Block::Heading(h) => self.inlines(doc, &h.content, scope, false),
                other => {
                    let mut sub = Writer::new(self.flavor, self.options);
                    sub.block(doc, other, scope, 4);
                    self.seen.extend(sub.seen);
                    sub.out.trim_end().to_string()
                }
            };
            if !text.trim().is_empty() {
                body.push(text.trim().to_string());
            }
        }
        let mut prefix = String::new();
        if note.kind == NoteKind::Comment {
            if let Some(author) = &note.author {
                prefix.push_str(&if self.md() {
                    format!("**{author}**")
                } else {
                    author.clone()
                });
            }
            if let Some(date) = &note.date {
                prefix.push_str(&format!(" ({date})"));
            }
            if let Some(anchor) = note.anchor.as_deref().filter(|a| !a.trim().is_empty()) {
                prefix.push_str(&format!(" on \"{}\"", one_line(anchor, 120)));
            }
            if !prefix.is_empty() {
                prefix = format!("{}: ", prefix.trim());
            }
        }
        let label = if self.md() {
            format!("[^{}]:", note.label)
        } else {
            format!("[{}]", note.label)
        };
        let text = body.join("\n\n").replace('\n', "\n    ");
        let _ = writeln!(self.out, "{label} {prefix}{text}");
    }

    fn group(&mut self, doc: &Document, group: &Group, scope: &Scope, heading_base: u32) {
        match group.kind {
            GroupKind::Notes => {
                let level = heading_level(1, heading_base);
                self.heading(level, group.title.as_deref().unwrap_or("Notes"));
                self.blocks(doc, &group.blocks, scope, u32::from(level));
            }
            GroupKind::TextBox | GroupKind::Quote => {
                let mut sub = self.nested();
                sub.blocks(doc, &group.blocks, scope, heading_base);
                self.absorb(&mut sub);
                let inner = sub.out.trim();
                if inner.is_empty() {
                    return;
                }
                if let Some(title) = &group.title {
                    let _ = writeln!(self.out, "> [{}]", one_line(title, 120));
                }
                for line in inner.lines() {
                    if line.is_empty() {
                        self.out.push_str(">\n");
                    } else {
                        let _ = writeln!(self.out, "> {line}");
                    }
                }
            }
            GroupKind::Header | GroupKind::Footer | GroupKind::Other => {
                let mut label = match group.kind {
                    GroupKind::Header => "Header".to_string(),
                    GroupKind::Footer => "Footer".to_string(),
                    _ => String::new(),
                };
                if let Some(title) = &group.title {
                    label = if label.is_empty() {
                        title.clone()
                    } else {
                        format!("{label} ({title})")
                    };
                }
                let simple = group.blocks.iter().filter(|b| !b.is_blank()).count() == 1
                    && matches!(
                        group.blocks.iter().find(|b| !b.is_blank()),
                        Some(Block::Paragraph(_))
                    );
                if simple {
                    let Some(Block::Paragraph(p)) = group.blocks.iter().find(|b| !b.is_blank())
                    else {
                        return;
                    };
                    let text = self.inlines(doc, &p.content, scope, false);
                    let text = one_line(&text, usize::MAX);
                    if label.is_empty() {
                        let _ = writeln!(self.out, "{text}");
                    } else if self.md() {
                        let _ = writeln!(self.out, "**{label}:** {text}");
                    } else {
                        let _ = writeln!(self.out, "{label}: {text}");
                    }
                } else {
                    if !label.is_empty() {
                        if self.md() {
                            let _ = writeln!(self.out, "**{label}:**");
                        } else {
                            let _ = writeln!(self.out, "{label}:");
                        }
                    }
                    self.blocks(doc, &group.blocks, scope, heading_base);
                }
            }
        }
    }

    // ----- parts ------------------------------------------------------------

    /// Where a part can be fetched: the extracted file, or its address.
    fn reference(&self, scope: &Scope, id: &PartId) -> String {
        let path = scope.part_path(id);
        self.options
            .extracted
            .get(&path)
            .cloned()
            .unwrap_or_else(|| scope.address.child(id.as_str()).to_string())
    }

    fn image(&mut self, doc: &Document, id: &PartId, alt: Option<&str>, scope: &Scope) -> String {
        let path = scope.part_path(id);
        self.seen.insert(path.clone());
        let part = doc.part(id.as_str());
        let mut reference = self.reference(scope, id);
        // Without doctopus an address is no use; a linked picture is
        // still found where it links to.
        if self.options.standalone
            && !self.options.extracted.contains_key(&path)
            && let Some(link) = part.and_then(|p| p.link.clone())
        {
            reference = link;
        }
        let alt = alt
            .map(|a| one_line(a, 200))
            .filter(|a| !a.is_empty())
            .or_else(|| part.and_then(|p| p.name.clone()))
            .unwrap_or_else(|| "image".into());
        let info = part.map(image_info).unwrap_or_default();
        if self.md() {
            let target = link_target(reference);
            let alt = alt.replace(['[', ']'], "");
            if info.is_empty() {
                format!("![{alt}]({target})")
            } else {
                format!("![{alt}]({target} \"{}\")", info.replace('"', "'"))
            }
        } else if info.is_empty() {
            format!("[image {id}: {alt}]")
        } else {
            format!("[image {id}: {alt} ({info})]")
        }
    }

    fn embed(&mut self, doc: &Document, id: &PartId, scope: &Scope) {
        let path = scope.part_path(id);
        let first_use = self.seen.insert(path.clone());
        let Some(part) = doc.part(id.as_str()) else {
            let _ = writeln!(self.out, "<embedded id=\"{id}\" note=\"missing part\"/>");
            return;
        };
        if self.options.standalone {
            self.linked_embed(part, &path);
            return;
        }
        let address = scope.address.child(id.as_str()).to_string();
        let mut attrs = vec![("id", id.to_string()), ("ref", self.reference(scope, id))];
        attrs.push(("type", part_type(part)));
        if let Some(name) = &part.name {
            attrs.push(("name", name.clone()));
        }
        attrs.push(("size", human_size(part.size)));
        let open = |attrs: &[(&str, String)]| {
            let mut tag = String::from("<embedded");
            for (k, v) in attrs {
                let _ = write!(tag, " {k}=\"{}\"", xml_escape(v));
            }
            tag
        };
        match &part.document {
            Some(child) if first_use && scope.depth < self.options.embed_depth => {
                let _ = writeln!(self.out, "{}>\n", open(&attrs));
                let child_scope = scope.child(id);
                if !child.warnings.is_empty() {
                    let _ = writeln!(self.out, "warnings: {}", child.warnings.join("; "));
                }
                self.document(child, &child_scope);
                // Blank lines around the content keep Markdown parsers from
                // swallowing it into the HTML-like tag.
                self.blank_line();
                self.out.push_str("</embedded>\n");
            }
            Some(_) => {
                let note = if first_use {
                    format!("read it with: doctopus read {}", shell_quote(&address))
                } else {
                    "shown above".to_string()
                };
                attrs.push(("note", note));
                let _ = writeln!(self.out, "{}/>", open(&attrs));
            }
            None => {
                let note = if let Some(err) = &part.error {
                    format!("could not be read: {err}")
                } else if let Some(link) = &part.link {
                    format!("linked, not embedded: {link}")
                } else {
                    format!(
                        "doctopus cannot read this type; save it with: doctopus extract {} -o <dir>",
                        shell_quote(&address)
                    )
                };
                attrs.push(("note", note));
                let _ = writeln!(self.out, "{}/>", open(&attrs));
            }
        }
    }

    /// An embedded part in standalone output: one line linking to the file
    /// it was saved as, which for a document is its own Markdown conversion.
    fn linked_embed(&mut self, part: &Part, path: &str) {
        let name = part.name.clone().unwrap_or_else(|| part.id.to_string());
        let linked = part.size == 0 && part.link.is_some();
        let label = if part.document.is_some() {
            "Embedded document"
        } else if linked {
            "Linked file"
        } else {
            "Embedded file"
        };
        let what = match &part.document {
            Some(child) => child.format.name.to_string(),
            None => part_type(part),
        };
        let info = if linked {
            what
        } else {
            format!("{what}, {}", human_size(part.size))
        };
        let mut line = match self.options.extracted.get(path) {
            Some(file) if self.md() => format!(
                "{label}: [{}]({} \"{}\")",
                name.replace(['[', ']'], ""),
                link_target(file.clone()),
                info.replace('"', "'")
            ),
            Some(file) => format!("{label}: {name} ({info}): {file}"),
            None => format!("{label}: {name} ({info})"),
        };
        if let Some(err) = &part.error {
            let _ = write!(line, "; it could not be read: {err}");
        } else if let Some(link) = &part.link {
            let _ = write!(line, "; it links to {link}");
        }
        let _ = writeln!(self.out, "{line}");
    }

    /// Parts that no content block references (floating pictures the reader
    /// could not anchor, workbook-level objects) still deserve a mention.
    fn unplaced_parts(&mut self, doc: &Document, scope: &Scope) {
        let unplaced: Vec<&Part> = doc
            .parts
            .iter()
            .filter(|p| !self.seen.contains(&scope.part_path(&p.id)))
            .collect();
        if unplaced.is_empty() {
            return;
        }
        self.blank_line();
        let level = heading_level(2, scope.depth);
        self.heading(level, "Other embedded content");
        for part in unplaced {
            self.blank_line();
            if part.kind == crate::PartKind::Image {
                let rendered = self.image(doc, &part.id, None, scope);
                self.out.push_str(&rendered);
                self.out.push('\n');
            } else {
                self.embed(doc, &part.id, scope);
            }
        }
    }

    // ----- inline content ---------------------------------------------------

    fn inlines(
        &mut self,
        doc: &Document,
        content: &[Inline],
        scope: &Scope,
        in_table: bool,
    ) -> String {
        let s = self.inlines_raw(doc, content, scope, in_table);
        collapse_spaces(&s)
    }

    fn inlines_raw(
        &mut self,
        doc: &Document,
        content: &[Inline],
        scope: &Scope,
        in_table: bool,
    ) -> String {
        let mut s = String::new();
        for inline in content {
            match inline {
                Inline::Text(t) => {
                    // Code spans show their text as it is.
                    let text = if t.style.code {
                        t.text.clone()
                    } else {
                        no_links(&t.text, self.md())
                    };
                    s.push_str(&self.styled(&text.replace('\t', " "), t.style));
                }
                Inline::Link(link) => {
                    let inner = self.inlines_raw(doc, &link.content, scope, in_table);
                    let heading = link
                        .url
                        .strip_prefix('#')
                        .and_then(|name| self.resolved.get(&scope.anchor(name)))
                        .map(|slug| format!("#{slug}"));
                    let url = heading.as_deref().unwrap_or(&link.url);
                    // Plain text has nowhere to jump to, and neither has
                    // standalone Markdown when no heading is the target.
                    // `read` keeps `#page-3`, which says what to select.
                    let nowhere = heading.is_none() && (!self.md() || self.options.standalone);
                    if url.starts_with('#') && nowhere {
                        s.push_str(&inner);
                    } else {
                        s.push_str(&self.link(&inner, url));
                    }
                }
                Inline::LineBreak => s.push_str(if in_table && self.md() { "<br>" } else { "\n" }),
                Inline::NoteRef(n) => {
                    if self.md() {
                        let _ = write!(s, "[^{}]", n.label);
                    } else {
                        let _ = write!(s, "[{}]", n.label);
                    }
                }
                Inline::Image(img) => {
                    let rendered = self.image(doc, &img.part, img.alt.as_deref(), scope);
                    s.push_str(&rendered);
                }
                Inline::Math(m) => {
                    if self.md() {
                        let _ = write!(s, "${}$", m.tex.trim());
                    } else {
                        s.push_str(m.tex.trim());
                    }
                }
            }
        }
        s
    }

    fn styled(&self, text: &str, style: Style) -> String {
        if !self.md() || style == Style::default() || text.trim().is_empty() {
            return text.to_string();
        }
        let start = text.len() - text.trim_start().len();
        let end = text.trim_end().len();
        let (lead, core, trail) = (&text[..start], &text[start..end], &text[end..]);
        let mut core = core.to_string();
        if style.code {
            core = code_span(&core);
        }
        if style.superscript {
            core = format!("<sup>{core}</sup>");
        } else if style.subscript {
            core = format!("<sub>{core}</sub>");
        }
        if style.strike {
            core = format!("~~{core}~~");
        }
        if style.italic {
            core = format!("*{core}*");
        }
        if style.bold {
            core = format!("**{core}**");
        }
        format!("{lead}{core}{trail}")
    }

    fn link(&self, text: &str, url: &str) -> String {
        let text = text.trim();
        if !self.md() {
            return if text.is_empty() || text == url {
                url.to_string()
            } else {
                format!("{text} <{url}>")
            };
        }
        if text.is_empty() || text == url {
            return format!("<{url}>");
        }
        let target = if url.contains([' ', '(', ')']) {
            format!("<{url}>")
        } else {
            url.to_string()
        };
        format!("[{}]({target})", text.replace(['[', ']'], ""))
    }
}

// ----- free helpers -----------------------------------------------------------

fn heading_level(level: u32, base: u32) -> u8 {
    (level + base).clamp(1, 6) as u8
}

fn section_title(section: &Section) -> Option<String> {
    let number = section.number.map(|n| format!(" {n}")).unwrap_or_default();
    let name = section.name.as_deref().map(|n| one_line(n, 200));
    let mut title = match section.kind {
        SectionKind::Body => return name,
        SectionKind::Sheet => format!("Sheet{number}"),
        SectionKind::Slide => format!("Slide{number}"),
        SectionKind::Page => format!("Page{number}"),
        SectionKind::HeadersFooters => return Some("Headers and footers".into()),
        SectionKind::Footnotes => return Some("Footnotes".into()),
        SectionKind::Endnotes => return Some("Endnotes".into()),
        SectionKind::Comments => return Some("Comments".into()),
        SectionKind::Other => return Some(name.unwrap_or_else(|| "Section".into())),
    };
    if let Some(name) = name {
        title.push_str(": ");
        title.push_str(&name);
    }
    if section.hidden {
        title.push_str(" (hidden)");
    }
    Some(title)
}

fn section_counts(doc: &Document) -> Vec<(&'static str, usize)> {
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    for section in doc.primary_sections() {
        let noun = match section.kind {
            SectionKind::Sheet => "sheets",
            SectionKind::Slide => "slides",
            _ => "pages",
        };
        match counts.iter_mut().find(|(n, _)| *n == noun) {
            Some((_, c)) => *c += 1,
            None => counts.push((noun, 1)),
        }
    }
    counts
}

/// Type of a part in a few words: the parsed document's format, the
/// container's description, or the media type.
pub(crate) fn part_type(part: &Part) -> String {
    if let Some(doc) = &part.document {
        return doc.format.id.to_string();
    }
    if let Some(ct) = &part.content_type {
        return ct.clone();
    }
    part.description.clone().unwrap_or_else(|| "unknown".into())
}

/// One-line description of a part for headers and listings.
pub fn describe_part(part: &Part) -> String {
    let mut out = match (&part.document, &part.description) {
        (Some(doc), _) => doc.format.name.to_string(),
        (None, Some(desc)) => desc.clone(),
        (None, None) => part
            .content_type
            .clone()
            .unwrap_or_else(|| format!("{:?}", part.kind).to_lowercase()),
    };
    if let Some(name) = &part.name {
        let _ = write!(out, " \"{}\"", one_line(name, 120));
    }
    let mut facts = Vec::new();
    if part.document.is_none()
        && let Some(ct) = &part.content_type
        && part.description.is_some()
    {
        facts.push(ct.clone());
    }
    if let Some((w, h)) = part.dimensions {
        facts.push(format!("{w}x{h}"));
    }
    facts.push(human_size(part.size));
    if let Some(loc) = &part.location {
        facts.push(loc.clone());
    }
    if let Some(link) = &part.link {
        facts.push(format!("linked to {link}"));
    }
    if part.error.is_some() {
        facts.push("unreadable".into());
    }
    let _ = write!(out, " ({})", facts.join(", "));
    out
}

fn image_info(part: &Part) -> String {
    let mut bits = Vec::new();
    if let Some(ext) = part
        .content_type
        .as_deref()
        .and_then(crate::sniff::extension_for_mime)
    {
        bits.push(ext.to_ascii_uppercase());
    }
    if let Some((w, h)) = part.dimensions {
        bits.push(format!("{w}x{h}"));
    }
    if part.size == 0 && part.link.is_some() {
        bits.push("linked".into());
    } else {
        bits.push(human_size(part.size));
    }
    bits.join(", ")
}

/// Flowchart labels of shapes, nested ones included: their text, else
/// their type, else their id.
fn shape_labels<'d>(shapes: &'d [Shape], out: &mut Vec<(&'d str, String)>) {
    for shape in shapes {
        let label = if !shape.text.trim().is_empty() {
            one_line(&shape.text, 60)
        } else if let Some(kind) = &shape.kind {
            kind.clone()
        } else {
            format!("shape {}", shape.id)
        };
        out.push((&shape.id, label));
        shape_labels(&shape.children, out);
    }
}

/// Text for a quoted Mermaid label. Characters that would end the label,
/// read as HTML or start an entity code are written as entity codes.
fn mermaid_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("#quot;"),
            '#' => out.push_str("#35;"),
            '<' => out.push_str("#lt;"),
            '>' => out.push_str("#gt;"),
            '|' => out.push_str("#124;"),
            _ => out.push(c),
        }
    }
    out
}

/// A code fence longer than any run of backticks in `text`, so the text
/// cannot close it.
fn fence(text: &str) -> String {
    "`".repeat((longest_run(text, '`') + 1).max(3))
}

/// A link destination, in angle brackets when it would otherwise end early.
fn link_target(reference: String) -> String {
    if reference.contains([' ', '(', ')', '<', '>']) {
        format!("<{reference}>")
    } else {
        reference
    }
}

fn shape_names(shapes: &[Shape]) -> Vec<(String, String)> {
    let mut names = Vec::new();
    for shape in shapes {
        if !shape.text.trim().is_empty() {
            names.push((
                shape.id.clone(),
                format!("\"{}\"", one_line(&shape.text, 60)),
            ));
        } else if let Some(kind) = &shape.kind {
            names.push((shape.id.clone(), format!("[{kind}]")));
        }
        names.extend(shape_names(&shape.children));
    }
    names
}

fn is_bullet(marker: &str) -> bool {
    let m = marker.trim();
    // Word's second-level bullet is a Courier New "o".
    m.is_empty() || m == "o" || m.chars().count() == 1 && !m.chars().all(char::is_alphanumeric)
}

/// Whether a list label is also a Markdown list marker: an ordered list
/// number (`3.`, `12)`) that viewers keep.
fn is_ordered_marker(label: &str) -> bool {
    let digits = label.trim_end_matches(['.', ')']);
    label.len() == digits.len() + 1
        && (1..=9).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Line breaks inside a paragraph as Markdown hard breaks (`\` at the end
/// of the line), so viewers keep them. Empty lines, which would end the
/// paragraph, are dropped.
fn hard_breaks(text: &str, markdown: bool) -> String {
    if !markdown || !text.contains('\n') {
        return text.to_string();
    }
    text.split('\n')
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\\\n")
}

/// Text as a Markdown code span, fenced with more backticks than it holds.
fn code_span(text: &str) -> String {
    let fence = "`".repeat(longest_run(text, '`') + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

/// The longest run of `c` in `text`.
fn longest_run(text: &str, c: char) -> usize {
    let (mut longest, mut run) = (0, 0);
    for ch in text.chars() {
        run = if ch == c { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

/// Whether the document, or a document embedded in it, links to a place
/// inside itself (`#name`).
fn has_internal_links(doc: &Document) -> bool {
    fn inlines(content: &[Inline]) -> bool {
        content
            .iter()
            .any(|i| matches!(i, Inline::Link(link) if link.url.starts_with('#')))
    }
    fn blocks(list: &[Block]) -> bool {
        list.iter().any(|block| match block {
            Block::Heading(h) => inlines(&h.content),
            Block::Paragraph(p) => inlines(&p.content),
            Block::ListItem(l) => inlines(&l.content),
            Block::Table(t) => t
                .rows
                .iter()
                .flat_map(|r| &r.cells)
                .any(|c| blocks(&c.blocks)),
            Block::Group(g) => blocks(&g.blocks),
            Block::Note(n) => blocks(&n.blocks),
            _ => false,
        })
    }
    doc.sections.iter().any(|s| blocks(&s.blocks))
        || doc
            .parts
            .iter()
            .filter_map(|p| p.document.as_deref())
            .any(has_internal_links)
}

/// Collapse runs of spaces inside lines (they appear where emphasis markers
/// moved surrounding whitespace, and carry no meaning for a reader).
fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut at_line_start = true;
    let mut prev_space = false;
    for c in s.chars() {
        if c == ' ' && prev_space && !at_line_start {
            continue;
        }
        prev_space = c == ' ';
        if c == '\n' {
            at_line_start = true;
        } else if c != ' ' {
            at_line_start = false;
        }
        out.push(c);
    }
    out
}

/// Collapse whitespace to single spaces and cut to `max` characters.
fn one_line(s: &str, max: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= max {
        joined
    } else {
        let cut: String = joined.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

/// Stop text that looks like `[label](target)`, such as the number format
/// `[Red]($#,##0.00)`, from being read as a link.
fn no_links(text: &str, markdown: bool) -> String {
    if markdown {
        text.replace("](", "\\](")
    } else {
        text.to_string()
    }
}

/// Stop paragraph text from being read as a heading or quote.
fn escape_line_start(text: &str, markdown: bool) -> String {
    if markdown && (text.starts_with('#') || text.starts_with('>')) {
        format!("\\{text}")
    } else {
        text.to_string()
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\n', " ")
}

fn to_char_offsets(text: &str, marks: Vec<(usize, SectionMark)>) -> Vec<SectionMark> {
    let mut out = Vec::with_capacity(marks.len());
    let mut chars = 0;
    let mut last_byte = 0;
    for (byte, mut mark) in marks {
        let byte = byte.min(text.len());
        if byte >= last_byte && text.is_char_boundary(byte) {
            chars += text[last_byte..byte].chars().count();
            last_byte = byte;
        }
        mark.offset = chars;
        out.push(mark);
    }
    out
}

#[cfg(test)]
mod tests;
