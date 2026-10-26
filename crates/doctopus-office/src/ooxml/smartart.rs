//! SmartArt: the data model behind a diagram (`dgm:dataModel`) read as a
//! nested list, which is how its author typed it in the text pane.

use std::collections::{HashMap, HashSet};

use doctopus_core::{Block, Inline, ListItem};

use super::xml::Element;

/// Read a SmartArt data model into list items.
pub(crate) fn read(model: &Element) -> Vec<Block> {
    let points: Vec<&Element> = model
        .child("dgm:ptLst")
        .map(|l| l.children_named("dgm:pt").collect())
        .unwrap_or_default();
    let mut text: HashMap<&str, Vec<String>> = HashMap::new();
    let mut root = None;
    for pt in &points {
        let Some(id) = pt.attr("modelId") else {
            continue;
        };
        match pt.attr("type").unwrap_or("node") {
            "doc" => root = Some(id),
            "node" | "asst" => {
                let paragraphs: Vec<String> = pt
                    .child("dgm:t")
                    .map(|t| {
                        t.children_named("a:p")
                            .map(|p| p.text().trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                text.insert(id, paragraphs);
            }
            _ => {}
        }
    }
    let mut children: HashMap<&str, Vec<(u32, &str)>> = HashMap::new();
    if let Some(list) = model.child("dgm:cxnLst") {
        for cxn in list.children_named("dgm:cxn") {
            if cxn.attr("type").unwrap_or("parOf") != "parOf" {
                continue;
            }
            let (Some(src), Some(dest)) = (cxn.attr("srcId"), cxn.attr("destId")) else {
                continue;
            };
            let ord = cxn.attr_parse("srcOrd").unwrap_or(0);
            children.entry(src).or_default().push((ord, dest));
        }
    }
    for list in children.values_mut() {
        list.sort_by_key(|(ord, _)| *ord);
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if let Some(root) = root {
        walk(root, 0, &children, &text, &mut seen, &mut out);
    }
    out
}

fn walk<'a>(
    id: &'a str,
    level: u8,
    children: &HashMap<&'a str, Vec<(u32, &'a str)>>,
    text: &HashMap<&'a str, Vec<String>>,
    seen: &mut HashSet<&'a str>,
    out: &mut Vec<Block>,
) {
    if !seen.insert(id) {
        return;
    }
    let Some(kids) = children.get(id) else {
        return;
    };
    for (_, child) in kids {
        let mut next_level = level;
        if let Some(paragraphs) = text.get(child).filter(|p| !p.is_empty()) {
            let mut content = Vec::new();
            for (i, p) in paragraphs.iter().enumerate() {
                if i > 0 {
                    content.push(Inline::LineBreak);
                }
                content.push(Inline::text(p.clone()));
            }
            out.push(Block::ListItem(ListItem {
                level,
                label: None,
                content,
            }));
            next_level = level.saturating_add(1).min(8);
        }
        walk(child, next_level, children, text, seen, out);
    }
}

#[cfg(test)]
mod tests {
    use doctopus_core::plain_text;

    use super::*;
    use crate::ooxml::xml::parse;

    #[test]
    fn builds_a_nested_list_in_order() {
        let xml =
            br#"<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"
                xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
            <dgm:ptLst>
              <dgm:pt modelId="0" type="doc"/>
              <dgm:pt modelId="1"><dgm:t><a:p><a:r><a:t>Plan</a:t></a:r></a:p></dgm:t></dgm:pt>
              <dgm:pt modelId="2"><dgm:t><a:p><a:r><a:t>Build</a:t></a:r></a:p></dgm:t></dgm:pt>
              <dgm:pt modelId="3"><dgm:t><a:p><a:r><a:t>Test it</a:t></a:r></a:p></dgm:t></dgm:pt>
              <dgm:pt modelId="9" type="parTrans"/>
            </dgm:ptLst>
            <dgm:cxnLst>
              <dgm:cxn modelId="10" srcId="0" destId="2" srcOrd="1"/>
              <dgm:cxn modelId="11" srcId="0" destId="1" srcOrd="0"/>
              <dgm:cxn modelId="12" srcId="2" destId="3" srcOrd="0"/>
              <dgm:cxn modelId="13" type="presOf" srcId="1" destId="9"/>
            </dgm:cxnLst>
        </dgm:dataModel>"#;
        let items: Vec<(u8, String)> = read(&parse(xml).root)
            .into_iter()
            .map(|b| match b {
                Block::ListItem(l) => (l.level, plain_text(&l.content)),
                _ => panic!("not a list item"),
            })
            .collect();
        assert_eq!(
            items,
            [
                (0, "Plan".to_string()),
                (0, "Build".to_string()),
                (1, "Test it".to_string())
            ]
        );
    }
}
