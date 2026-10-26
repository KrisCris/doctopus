//! Narrowing a document down to the sheets, slides, pages or cells a caller
//! asked for.

use crate::{Block, CellRange, Document, Error, Result, Section};

/// One term of a section selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionSelector {
    /// A 1-based section number.
    Number(u32),
    /// An inclusive range of section numbers.
    Range(u32, u32),
    /// A section name, case-insensitive; `*` matches any run of characters.
    Name(String),
}

impl SectionSelector {
    /// Parse a comma-separated list such as `2`, `1,3-5` or `Summary,Q*`.
    pub fn parse_list(spec: &str) -> Result<Vec<Self>> {
        let terms: Vec<Self> = spec
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(Self::parse_term)
            .collect();
        if terms.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "empty selection `{spec}`; use a number (3), a range (2-5) or a name"
            )));
        }
        Ok(terms)
    }

    fn parse_term(term: &str) -> Self {
        if let Ok(n) = term.parse::<u32>() {
            return Self::Number(n);
        }
        if let Some((a, b)) = term.split_once('-')
            && let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>())
        {
            return Self::Range(a.min(b), a.max(b));
        }
        Self::Name(term.to_string())
    }

    fn matches(&self, section: &Section) -> bool {
        match self {
            Self::Number(n) => section.number == Some(*n),
            Self::Range(a, b) => section.number.is_some_and(|n| (*a..=*b).contains(&n)),
            Self::Name(pattern) => section
                .name
                .as_deref()
                .is_some_and(|name| wildcard_match(pattern, name)),
        }
    }
}

/// What part of a document to keep.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Sheets, slides or pages to keep; empty keeps all.
    pub sections: Vec<SectionSelector>,
    /// Spreadsheet cells to keep.
    pub cells: Option<CellRange>,
}

impl Selection {
    /// Whether the selection keeps everything.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty() && self.cells.is_none()
    }

    /// Apply the selection in place.
    ///
    /// Errors explain what *can* be selected, so a caller can correct itself.
    pub fn apply(&self, doc: &mut Document) -> Result<()> {
        if !self.sections.is_empty() {
            let available = describe_sections(doc);
            if available.is_empty() {
                return Err(Error::InvalidArgument(format!(
                    "this {} has no sheets, slides or pages to select; page through it with --offset instead",
                    doc.format.name
                )));
            }
            doc.sections
                .retain(|s| s.kind.is_primary() && self.sections.iter().any(|sel| sel.matches(s)));
            if doc.sections.is_empty() {
                return Err(Error::InvalidArgument(format!(
                    "no {} matches the selection; available: {}",
                    noun_of(&available),
                    available.join(", ")
                )));
            }
        }
        if let Some(range) = self.cells {
            let mut any_grid = false;
            for section in &mut doc.sections {
                for block in &mut section.blocks {
                    if let Block::Grid(grid) = block {
                        any_grid = true;
                        for row in &mut grid.rows {
                            row.cells.retain(|c| range.contains(row.row, c.col));
                        }
                        grid.rows.retain(|r| !r.cells.is_empty());
                        grid.merged.retain(|m| {
                            range.contains(m.start.row, m.start.col)
                                || range.contains(m.end.row, m.end.col)
                        });
                    }
                }
                section
                    .properties
                    .insert("selected_range".into(), range.to_string());
            }
            if !any_grid {
                return Err(Error::InvalidArgument(format!(
                    "--range applies to spreadsheets, but this is a {}",
                    doc.format.name
                )));
            }
        }
        Ok(())
    }
}

fn describe_sections(doc: &Document) -> Vec<String> {
    doc.primary_sections()
        .map(|s| {
            let number = s.number.map(|n| n.to_string()).unwrap_or_default();
            match &s.name {
                Some(name) => format!("{} {number} \"{name}\"", s.kind.noun()),
                None => format!("{} {number}", s.kind.noun()),
            }
        })
        .collect()
}

fn noun_of(available: &[String]) -> &str {
    available
        .first()
        .and_then(|a| a.split(' ').next())
        .unwrap_or("section")
}

/// Case-insensitive match where `*` stands for any run of characters.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_selector_lists() {
        assert_eq!(
            SectionSelector::parse_list("1, 3-5,Summary").unwrap(),
            vec![
                SectionSelector::Number(1),
                SectionSelector::Range(3, 5),
                SectionSelector::Name("Summary".into())
            ]
        );
        assert!(SectionSelector::parse_list(" , ").is_err());
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("q*", "Q3 Results"));
        assert!(wildcard_match("*results", "Q3 Results"));
        assert!(wildcard_match("summary", "Summary"));
        assert!(!wildcard_match("sum", "Summary"));
    }
}
