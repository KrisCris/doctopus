//! DrawingML charts (`c:chartSpace`) and Office 2016 charts
//! (`cx:chartSpace`, waterfall, treemap, funnel, ...).
//!
//! Charts keep a cache of the values they display next to the reference to
//! the workbook range they come from. The cache is what is read: it is what
//! the reader of the document sees, and it is there even when the source
//! workbook is not.

use doctopus_core::{Block, Chart, Series, format_number};

use super::xml::Element;
use crate::dates::excel_date;

/// Read a `c:chartSpace` part.
pub(crate) fn read(root: &Element) -> Option<Block> {
    let chart = root.child("c:chart")?;
    let title = chart.child("c:title").and_then(title_text);
    let plot = chart.child("c:plotArea")?;
    let mut kinds: Vec<String> = Vec::new();
    let mut categories: Vec<String> = Vec::new();
    let mut series = Vec::new();
    for group in plot.children().filter(|g| g.local().ends_with("Chart")) {
        let kind = chart_kind(group);
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
        for ser in group.children_named("c:ser") {
            let name = ser
                .child("c:tx")
                .and_then(|tx| strings(tx).into_iter().next().or_else(|| Some(tx.text())))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let cat = ser.child("c:cat").or_else(|| ser.child("c:xVal"));
            if categories.is_empty()
                && let Some(cat) = cat
            {
                categories = strings(cat);
            }
            let values = ser
                .child("c:val")
                .or_else(|| ser.child("c:yVal"))
                .map(numbers)
                .unwrap_or_default();
            series.push(Series { name, values });
        }
    }
    if series.is_empty() && title.is_none() {
        return None;
    }
    Some(Block::Chart(Chart {
        title,
        kind: if kinds.is_empty() {
            "chart".into()
        } else {
            kinds.join(" + ")
        },
        categories,
        series,
    }))
}

fn chart_kind(group: &Element) -> String {
    let base = group.local().trim_end_matches("Chart");
    let base = base.trim_end_matches("3D");
    match base {
        "bar" => match group.child_val("c:barDir") {
            Some("col") => "column".into(),
            _ => "bar".into(),
        },
        "ofPie" => "pie of pie".into(),
        other => other.to_string(),
    }
}

fn title_text(title: &Element) -> Option<String> {
    let tx = title.child("c:tx")?;
    let text = if let Some(rich) = tx.child("c:rich") {
        rich.children_named("a:p")
            .map(|p| p.text())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        strings(tx).join(" ")
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

/// Point values of a cache (`c:strCache`, `c:numCache`, or the literal
/// `c:strLit`/`c:numLit`), placed by index.
fn cache_points(cache: &Element) -> Vec<(usize, String)> {
    cache
        .children_named("c:pt")
        .filter_map(|pt| {
            let idx = pt.attr_parse::<usize>("idx")?;
            let v = pt.child("c:v")?.text();
            Some((idx, v))
        })
        .collect()
}

fn find_cache(el: &Element) -> Option<&Element> {
    el.descendants().find(|e| {
        matches!(
            e.name(),
            "c:strCache" | "c:numCache" | "c:strLit" | "c:numLit" | "c:multiLvlStrCache"
        )
    })
}

fn point_count(cache: &Element) -> usize {
    cache
        .child("c:ptCount")
        .and_then(|c| c.attr_parse::<usize>("val"))
        .unwrap_or(0)
}

/// Category labels (or series names): strings by index. Multi-level
/// categories are joined innermost first (`Q1 / 2024`).
fn strings(el: &Element) -> Vec<String> {
    let Some(cache) = find_cache(el) else {
        return Vec::new();
    };
    if cache.is("c:multiLvlStrCache") {
        let count = point_count(cache);
        let mut labels = vec![String::new(); count.min(MAX_POINTS)];
        for lvl in cache.children_named("c:lvl") {
            for (idx, v) in cache_points(lvl) {
                if let Some(label) = labels.get_mut(idx) {
                    if label.is_empty() {
                        *label = v;
                    } else if !v.is_empty() {
                        label.push_str(" / ");
                        label.push_str(&v);
                    }
                }
            }
        }
        // Outer levels only label the first point of their span; that is
        // fine for reading.
        return labels;
    }
    let format = cache.child("c:formatCode").map(|f| f.text());
    let points = cache_points(cache);
    let count = point_count(cache).max(points.iter().map(|(i, _)| i + 1).max().unwrap_or(0));
    let mut out = vec![String::new(); count.min(MAX_POINTS)];
    for (idx, v) in points {
        if let Some(slot) = out.get_mut(idx) {
            *slot = match (
                cache.is("c:numCache") || cache.is("c:numLit"),
                v.parse::<f64>(),
            ) {
                (true, Ok(n)) if format.as_deref().is_some_and(is_date_format) => {
                    excel_date(n).unwrap_or(v)
                }
                (true, Ok(n)) => format_number(n),
                _ => v,
            };
        }
    }
    out
}

/// Charts with more points than this are cut off; their table would be
/// unreadable anyway.
const MAX_POINTS: usize = 10_000;

fn numbers(el: &Element) -> Vec<Option<f64>> {
    let Some(cache) = find_cache(el) else {
        return Vec::new();
    };
    let points = cache_points(cache);
    let count = point_count(cache).max(points.iter().map(|(i, _)| i + 1).max().unwrap_or(0));
    let mut out = vec![None; count.min(MAX_POINTS)];
    for (idx, v) in points {
        if let Some(slot) = out.get_mut(idx) {
            *slot = v.trim().parse().ok();
        }
    }
    out
}

fn is_date_format(code: &str) -> bool {
    let lower = code.to_ascii_lowercase();
    (lower.contains('y') || lower.contains('d') || lower.contains("mmm")) && !lower.contains('#')
}

/// Read a `cx:chartSpace` part (Office 2016 chart types).
pub(crate) fn read_chartex(root: &Element) -> Option<Block> {
    let chart = root.child("cx:chart")?;
    let title = chart
        .child("cx:title")
        .map(|t| t.text().split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty());
    let data = root.child("cx:chartData");
    let mut kinds = Vec::new();
    let mut categories = Vec::new();
    let mut series = Vec::new();
    for ser in chart.find_all("cx:series") {
        if let Some(layout) = ser.attr("layoutId") {
            let layout = layout.to_string();
            if !kinds.contains(&layout) {
                kinds.push(layout);
            }
        }
        let name = ser
            .child("cx:tx")
            .map(|t| t.text().trim().to_string())
            .filter(|s| !s.is_empty());
        let Some(data_id) = ser.child("cx:dataId").and_then(|d| d.attr("val")) else {
            continue;
        };
        let Some(set) = data.and_then(|d| {
            d.children_named("cx:data")
                .find(|x| x.attr("id") == Some(data_id))
        }) else {
            continue;
        };
        if categories.is_empty()
            && let Some(dim) = set.child("cx:strDim")
        {
            categories = chartex_level(dim)
                .into_iter()
                .map(|v| v.unwrap_or_default())
                .collect();
        }
        let values = set
            .child("cx:numDim")
            .map(|dim| {
                chartex_level(dim)
                    .into_iter()
                    .map(|v| v.and_then(|s| s.trim().parse().ok()))
                    .collect()
            })
            .unwrap_or_default();
        series.push(Series { name, values });
    }
    if series.is_empty() && title.is_none() {
        return None;
    }
    Some(Block::Chart(Chart {
        title,
        kind: if kinds.is_empty() {
            "chart".into()
        } else {
            kinds.join(" + ")
        },
        categories,
        series,
    }))
}

fn chartex_level(dim: &Element) -> Vec<Option<String>> {
    let Some(lvl) = dim.find("cx:lvl") else {
        return Vec::new();
    };
    let count = lvl
        .attr_parse::<usize>("ptCount")
        .unwrap_or(0)
        .min(MAX_POINTS);
    let mut out = vec![None; count];
    for pt in lvl.children_named("cx:pt") {
        let Some(idx) = pt.attr_parse::<usize>("idx") else {
            continue;
        };
        if idx >= out.len() && idx < MAX_POINTS {
            out.resize(idx + 1, None);
        }
        if let Some(slot) = out.get_mut(idx) {
            *slot = Some(pt.text());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ooxml::xml::parse;

    #[test]
    fn reads_cached_series() {
        let xml = br#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"
              xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
          <c:chart>
            <c:title><c:tx><c:rich><a:p><a:r><a:t>Sales</a:t></a:r></a:p></c:rich></c:tx></c:title>
            <c:plotArea><c:barChart><c:barDir val="col"/>
              <c:ser><c:tx><c:strRef><c:f>Sheet1!$B$1</c:f><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>2024</c:v></c:pt></c:strCache></c:strRef></c:tx>
                <c:cat><c:strRef><c:strCache><c:ptCount val="2"/><c:pt idx="0"><c:v>North</c:v></c:pt><c:pt idx="1"><c:v>South</c:v></c:pt></c:strCache></c:strRef></c:cat>
                <c:val><c:numRef><c:numCache><c:ptCount val="2"/><c:pt idx="1"><c:v>7.5</c:v></c:pt></c:numCache></c:numRef></c:val>
              </c:ser></c:barChart></c:plotArea>
          </c:chart></c:chartSpace>"#;
        let Some(Block::Chart(chart)) = read(&parse(xml).root) else {
            panic!("no chart");
        };
        assert_eq!(chart.title.as_deref(), Some("Sales"));
        assert_eq!(chart.kind, "column");
        assert_eq!(chart.categories, ["North", "South"]);
        assert_eq!(chart.series[0].name.as_deref(), Some("2024"));
        assert_eq!(chart.series[0].values, [None, Some(7.5)]);
    }
}
