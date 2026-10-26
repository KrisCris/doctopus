//! Document properties: `docProps/core.xml` (Dublin Core), `app.xml`
//! (application statistics) and `custom.xml` (user-defined properties).

use doctopus_core::Document;

use super::package::Package;
use crate::ole::props::is_bookkeeping;

/// Read every property part the package declares into `doc.metadata`.
pub(crate) fn read(pkg: &mut Package<'_>, doc: &mut Document) {
    let rels = pkg.rels("");
    if let Some(core) = rels.first_target("core-properties").map(str::to_string) {
        read_core(pkg, &core, doc);
    }
    if let Some(app) = rels.first_target("extended-properties").map(str::to_string) {
        read_app(pkg, &app, doc);
    }
    if let Some(custom) = rels.first_target("custom-properties").map(str::to_string) {
        read_custom(pkg, &custom, doc);
    }
}

fn read_core(pkg: &mut Package<'_>, part: &str, doc: &mut Document) {
    let Some(root) = pkg.xml(part, doc) else {
        return;
    };
    for el in root.children() {
        let key = match el.name() {
            "dc:title" => "title",
            "dc:subject" => "subject",
            "dc:creator" => "author",
            "cp:lastModifiedBy" => "last_modified_by",
            "dcterms:created" => "created",
            "dcterms:modified" => "modified",
            "cp:keywords" => "keywords",
            "dc:description" => "description",
            "cp:category" => "category",
            "cp:contentStatus" => "status",
            "dc:language" => "language",
            "cp:revision" => "revision",
            _ => continue,
        };
        let value = el.text();
        if matches!(key, "created" | "modified") && !plausible_date(&value) {
            continue;
        }
        doc.metadata.set(key, value);
    }
}

/// Generators write zero timestamps (`1601-01-01`, `1899-12-30`) for
/// "unknown"; no Office document predates 1980.
pub(crate) fn plausible_date(value: &str) -> bool {
    value
        .trim()
        .get(..4)
        .and_then(|y| y.parse::<u32>().ok())
        .is_some_and(|year| year >= 1980)
}

fn read_app(pkg: &mut Package<'_>, part: &str, doc: &mut Document) {
    let Some(root) = pkg.xml(part, doc) else {
        return;
    };
    for el in root.children() {
        let value = el.text();
        let key = match el.local() {
            "Application" => "application",
            "Company" => "company",
            "Manager" => "manager",
            "Template" => "template",
            "Pages" => "pages",
            "Words" => "words",
            "Characters" => "characters",
            "TotalTime" => {
                if let Ok(minutes) = value.trim().parse::<u64>()
                    && minutes > 0
                {
                    doc.metadata
                        .set("total_editing_time", format!("{minutes} minutes"));
                }
                continue;
            }
            _ => continue,
        };
        // Statistics of zero are placeholders written by generators.
        if matches!(key, "pages" | "words" | "characters") && value.trim() == "0" {
            continue;
        }
        doc.metadata.set(key, value);
    }
}

fn read_custom(pkg: &mut Package<'_>, part: &str, doc: &mut Document) {
    let Some(root) = pkg.xml(part, doc) else {
        return;
    };
    for prop in root.children().filter(|e| e.local() == "property") {
        let Some(name) = prop.attr("name") else {
            continue;
        };
        if is_bookkeeping(name) {
            continue;
        }
        let Some(value) = prop.children().next() else {
            continue;
        };
        doc.metadata.set(&format!("custom.{name}"), value.text());
    }
}
