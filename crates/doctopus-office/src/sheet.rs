//! Turning calamine's cell ranges into the document model's [`Grid`]; shared
//! by the Excel readers (`.xlsx`, `.xlsb`, `.xls`).

use std::collections::HashMap;

use calamine::{Data, Dimensions, ExcelDateTime, Range};
use doctopus_core::{CellRange, CellRef, Grid, GridCell, GridRow, Value};

/// Cells kept per sheet; larger sheets are cut off with a warning, so a
/// pathological file cannot exhaust memory.
pub(crate) const MAX_CELLS: usize = 2_000_000;

/// Everything known about the cells of one sheet. Values and formulas are
/// kept sparse: a sheet with two cells far apart stays two cells.
#[derive(Debug, Default)]
pub(crate) struct SheetCells {
    /// Values by position, in reading order.
    values: Vec<((u32, u32), Data)>,
    /// Formulas by position, without the leading `=`.
    formulas: HashMap<(u32, u32), String>,
    pub(crate) merged: Vec<Dimensions>,
    /// Hyperlink targets by (row, column).
    pub(crate) links: HashMap<(u32, u32), String>,
    /// Note or comment text by (row, column).
    pub(crate) comments: HashMap<(u32, u32), String>,
    /// Whether cells were left out for reaching [`MAX_CELLS`].
    truncated: bool,
}

impl SheetCells {
    /// Add a cell's value. `false` once [`MAX_CELLS`] values are held:
    /// the value is then left out, and so should the rest be.
    pub(crate) fn add_value(&mut self, pos: (u32, u32), value: Data) -> bool {
        if value == Data::Empty {
            return true;
        }
        if self.values.len() >= MAX_CELLS {
            self.truncated = true;
            return false;
        }
        self.values.push((pos, value));
        true
    }

    /// Add a cell's formula; like [`Self::add_value`].
    pub(crate) fn add_formula(&mut self, pos: (u32, u32), formula: &str) -> bool {
        let formula = formula.trim_start_matches('=');
        if formula.is_empty() {
            return true;
        }
        if self.formulas.len() >= MAX_CELLS {
            self.truncated = true;
            return false;
        }
        self.formulas.insert(pos, formula.to_string());
        true
    }

    /// Add the values of a sheet read whole.
    pub(crate) fn add_values(&mut self, range: &Range<Data>) {
        let Some((r0, c0)) = range.start() else {
            return;
        };
        for (r, c, data) in range.used_cells() {
            if !self.add_value((r0 + r as u32, c0 + c as u32), data.clone()) {
                break;
            }
        }
    }

    /// Add the formulas of a sheet read whole.
    pub(crate) fn add_formulas(&mut self, range: &Range<String>) {
        let Some((r0, c0)) = range.start() else {
            return;
        };
        for (r, c, formula) in range.used_cells() {
            if !self.add_formula((r0 + r as u32, c0 + c as u32), formula) {
                break;
            }
        }
    }
}

/// The result of building a grid.
#[derive(Debug)]
pub(crate) struct Built {
    pub(crate) grid: Grid,
    /// Whether [`MAX_CELLS`] was reached.
    pub(crate) truncated: bool,
}

/// Build the grid of a sheet.
pub(crate) fn build(cells: SheetCells) -> Built {
    let mut by_row: std::collections::BTreeMap<u32, Vec<GridCell>> =
        std::collections::BTreeMap::new();
    let mut count = 0usize;
    let mut truncated = cells.truncated;
    let mut seen: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    for (pos, data) in &cells.values {
        if count >= MAX_CELLS {
            truncated = true;
            break;
        }
        let value = convert(data);
        if (value == Value::Empty && !cells.formulas.contains_key(pos)) || !seen.insert(*pos) {
            continue;
        }
        by_row
            .entry(pos.0)
            .or_default()
            .push(cell(*pos, value, &cells));
        count += 1;
    }
    // Formulas, links and comments on cells without a cached value.
    let mut extra: Vec<(u32, u32)> = cells
        .formulas
        .keys()
        .chain(cells.links.keys())
        .chain(cells.comments.keys())
        .copied()
        .filter(|p| !seen.contains(p))
        .collect();
    extra.sort_unstable();
    for pos in extra {
        if !seen.insert(pos) {
            continue;
        }
        if count >= MAX_CELLS {
            truncated = true;
            break;
        }
        by_row
            .entry(pos.0)
            .or_default()
            .push(cell(pos, Value::Empty, &cells));
        count += 1;
    }
    let rows: Vec<GridRow> = by_row
        .into_iter()
        .map(|(row, mut cells)| {
            cells.sort_by_key(|c| c.col);
            GridRow { row, cells }
        })
        .collect();
    let dimension = bounds(&rows);
    let merged = cells
        .merged
        .iter()
        .map(|d| {
            CellRange::new(
                CellRef::new(d.start.0, d.start.1),
                CellRef::new(d.end.0, d.end.1),
            )
        })
        .collect();
    Built {
        grid: Grid {
            dimension,
            rows,
            merged,
        },
        truncated,
    }
}

fn cell(pos: (u32, u32), value: Value, cells: &SheetCells) -> GridCell {
    let mut cell = GridCell::new(pos.1, value);
    cell.formula = cells.formulas.get(&pos).cloned();
    cell.link = cells.links.get(&pos).cloned();
    cell.comment = cells.comments.get(&pos).cloned();
    cell
}

fn bounds(rows: &[GridRow]) -> Option<CellRange> {
    let first = rows.first()?;
    let last = rows.last()?;
    let (min_col, max_col) = rows
        .iter()
        .flat_map(|r| r.cells.iter().map(|c| c.col))
        .fold((u32::MAX, 0), |(lo, hi), c| (lo.min(c), hi.max(c)));
    Some(CellRange::new(
        CellRef::new(first.row, min_col),
        CellRef::new(last.row, max_col),
    ))
}

/// Convert one calamine value.
pub(crate) fn convert(data: &Data) -> Value {
    match data {
        Data::Empty => Value::Empty,
        Data::String(s) if s.is_empty() => Value::Empty,
        Data::String(s) => Value::Text(s.clone()),
        Data::Int(i) => Value::Number(*i as f64),
        Data::Float(f) => Value::Number(*f),
        Data::Bool(b) => Value::Bool(*b),
        Data::DateTime(dt) => Value::DateTime(format_datetime(dt)),
        Data::DateTimeIso(s) | Data::DurationIso(s) => Value::DateTime(s.clone()),
        Data::Error(e) => Value::Error(e.to_string()),
    }
}

/// Dates as `2024-01-31`, times as `14:05`, both as `2024-01-31 14:05`,
/// durations as `36:15:00`. Seconds are shown only when not zero.
fn format_datetime(dt: &ExcelDateTime) -> String {
    let serial = dt.as_f64();
    if dt.is_duration() {
        let total = (serial * 86_400.0).round() as i64;
        let (sign, total) = if total < 0 {
            ("-", -total)
        } else {
            ("", total)
        };
        return format!(
            "{sign}{}:{:02}:{:02}",
            total / 3600,
            total / 60 % 60,
            total % 60
        );
    }
    let (y, m, d, hh, mm, ss, _) = dt.to_ymd_hms_milli();
    let time = if ss > 0 {
        format!("{hh:02}:{mm:02}:{ss:02}")
    } else {
        format!("{hh:02}:{mm:02}")
    };
    let has_time = serial.fract().abs() > 1e-9;
    if serial.abs() < 1.0 && has_time {
        time
    } else if has_time {
        format!("{y:04}-{m:02}-{d:02} {time}")
    } else {
        format!("{y:04}-{m:02}-{d:02}")
    }
}

/// Decode a header or footer: `&L`, `&C` and `&R` start the left, centre
/// and right parts, fields such as `&P` become placeholders like `[page]`,
/// and formatting codes are dropped.
pub(crate) fn header_footer_text(code: &str) -> String {
    let mut parts = [String::new(), String::new(), String::new()];
    let mut current = 1;
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '&' {
            parts[current].push(c);
            continue;
        }
        let Some(code) = chars.next() else {
            break;
        };
        let field = match code {
            '&' => "&",
            'L' => {
                current = 0;
                ""
            }
            'C' => {
                current = 1;
                ""
            }
            'R' => {
                current = 2;
                ""
            }
            'P' => "[page]",
            'N' => "[pages]",
            'D' => "[date]",
            'T' => "[time]",
            'F' => "[file]",
            'A' => "[sheet]",
            'Z' => "[path]",
            'G' => "[picture]",
            // &"font,style"
            '"' => {
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                }
                ""
            }
            // &KRRGGBB or &KTTSNNN: text colour.
            'K' => {
                for _ in 0..6 {
                    chars.next();
                }
                ""
            }
            // &12: font size.
            d if d.is_ascii_digit() => {
                while chars.next_if(char::is_ascii_digit).is_some() {}
                ""
            }
            // Bold, italic, underline, strike-through and the like.
            _ => "",
        };
        parts[current].push_str(field);
    }
    let parts: Vec<(&str, &str)> = ["left", "center", "right"]
        .into_iter()
        .zip(parts.iter().map(|p| p.trim()))
        .filter(|(_, text)| !text.is_empty())
        .collect();
    match parts.as_slice() {
        [(_, text)] => (*text).to_string(),
        _ => parts
            .iter()
            .map(|(side, text)| format!("{side}: {text}"))
            .collect::<Vec<_>>()
            .join("; "),
    }
}

#[cfg(test)]
mod tests {
    use calamine::{Cell, ExcelDateTimeType};

    use super::*;

    #[test]
    fn builds_sparse_rows_with_formulas_links_and_comments() {
        let values = Range::from_sparse(vec![
            Cell::new((1, 1), Data::String("Total".into())),
            Cell::new((1, 2), Data::Float(12.5)),
            Cell::new((3, 0), Data::Bool(true)),
        ]);
        let formulas = Range::from_sparse(vec![Cell::new((1, 2), "SUM(C1:C1)".to_string())]);
        let mut cells = SheetCells::default();
        cells.add_values(&values);
        cells.add_formulas(&formulas);
        cells.links.insert((1, 1), "https://example.com".into());
        cells.comments.insert((5, 5), "Alice: check".into());
        let built = build(cells);
        let rows: Vec<u32> = built.grid.rows.iter().map(|r| r.row).collect();
        assert_eq!(rows, [1, 3, 5]);
        let total = &built.grid.rows[0].cells[1];
        assert_eq!(total.value, Value::Number(12.5));
        assert_eq!(total.formula.as_deref(), Some("SUM(C1:C1)"));
        assert_eq!(
            built.grid.rows[0].cells[0].link.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            built.grid.rows[2].cells[0].comment.as_deref(),
            Some("Alice: check")
        );
        assert_eq!(built.grid.dimension.unwrap().to_string(), "A2:F6");
    }

    #[test]
    fn formats_dates_times_and_durations() {
        let dt = |v: f64, t| format_datetime(&ExcelDateTime::new(v, t, false));
        assert_eq!(dt(45_292.0, ExcelDateTimeType::DateTime), "2024-01-01");
        assert_eq!(
            dt(45_292.5, ExcelDateTimeType::DateTime),
            "2024-01-01 12:00"
        );
        assert_eq!(dt(0.25, ExcelDateTimeType::DateTime), "06:00");
        assert_eq!(dt(1.5, ExcelDateTimeType::TimeDelta), "36:00:00");
    }

    #[test]
    fn decodes_header_footer_codes() {
        assert_eq!(
            header_footer_text("&CPage &P of &N"),
            "Page [page] of [pages]"
        );
        assert_eq!(
            header_footer_text("&L&\"Arial,Bold\"&12Report&C&KFF0000Draft&R&D &&co"),
            "left: Report; center: Draft; right: [date] &co"
        );
        assert_eq!(header_footer_text("&B&I"), "");
    }
}
