//! Open Packaging Conventions: the zip container behind every Office Open
//! XML file, its content types and the relationships between its parts.

use std::collections::HashMap;

use doctopus_core::container::Zip;
use doctopus_core::{Document, Error, FormatInfo, Result};

use super::xml::{self, Element};

/// One relationship from a part (or the package) to a target.
#[derive(Debug, Clone)]
pub(crate) struct Relationship {
    /// The id referenced from content (`rId3`).
    pub(crate) id: String,
    /// The relationship type URI.
    pub(crate) rel_type: String,
    /// For internal targets, the absolute part name without a leading `/`
    /// (`word/media/image1.png`); for external ones, the target as written.
    pub(crate) target: String,
    /// Whether the target lives outside the package (a URL or a file path).
    pub(crate) external: bool,
}

impl Relationship {
    /// The last segment of the type URI (`image`, `oleObject`, `hyperlink`),
    /// which is the same for transitional, strict and Microsoft variants.
    pub(crate) fn kind(&self) -> &str {
        self.rel_type.rsplit('/').next().unwrap_or(&self.rel_type)
    }
}

/// The relationships of one part.
#[derive(Debug, Clone, Default)]
pub(crate) struct Relationships {
    rels: Vec<Relationship>,
}

impl Relationships {
    /// Look up a relationship by id.
    pub(crate) fn get(&self, id: &str) -> Option<&Relationship> {
        self.rels.iter().find(|r| r.id == id)
    }

    /// The target of an internal relationship.
    pub(crate) fn target(&self, id: &str) -> Option<&str> {
        self.get(id)
            .filter(|r| !r.external)
            .map(|r| r.target.as_str())
    }

    /// All relationships of a kind (see [`Relationship::kind`]), in order.
    pub(crate) fn of_kind<'a>(
        &'a self,
        kind: &'a str,
    ) -> impl Iterator<Item = &'a Relationship> + 'a {
        self.rels.iter().filter(move |r| r.kind() == kind)
    }

    /// The first internal target of a kind.
    pub(crate) fn first_target(&self, kind: &str) -> Option<&str> {
        self.rels
            .iter()
            .find(|r| !r.external && r.kind() == kind)
            .map(|r| r.target.as_str())
    }

    /// Every relationship.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Relationship> {
        self.rels.iter()
    }
}

/// An opened package.
pub(crate) struct Package<'a> {
    zip: Zip<'a>,
    format: &'static FormatInfo,
    defaults: HashMap<String, String>,
    overrides: HashMap<String, String>,
    rels: HashMap<String, Relationships>,
}

impl std::fmt::Debug for Package<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Package")
            .field("format", &self.format.id)
            .field("parts", &self.zip.names().len())
            .finish_non_exhaustive()
    }
}

impl<'a> Package<'a> {
    /// Open a package and read its content types.
    pub(crate) fn open(
        data: &'a [u8],
        format: &'static FormatInfo,
        max_part_size: u64,
    ) -> Result<Self> {
        let zip = Zip::new(data, max_part_size)
            .map_err(|e| Error::malformed(format.name, format!("not a valid zip archive: {e}")))?;
        let mut package = Self {
            zip,
            format,
            defaults: HashMap::new(),
            overrides: HashMap::new(),
            rels: HashMap::new(),
        };
        if package.zip.contains("EncryptedPackage") {
            return Err(Error::encrypted(format.name));
        }
        if let Ok(Some(bytes)) = package.zip.read("[Content_Types].xml") {
            let types = xml::parse(&bytes).root;
            for el in types.children() {
                match el.local() {
                    "Default" => {
                        if let (Some(ext), Some(ct)) =
                            (el.attr("Extension"), el.attr("ContentType"))
                        {
                            package
                                .defaults
                                .insert(ext.to_ascii_lowercase(), ct.to_string());
                        }
                    }
                    "Override" => {
                        if let (Some(name), Some(ct)) =
                            (el.attr("PartName"), el.attr("ContentType"))
                        {
                            package.overrides.insert(normalize(name), ct.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(package)
    }

    /// Whether the archive was damaged and only partly recovered.
    pub(crate) fn is_recovered(&self) -> bool {
        self.zip.is_recovered()
    }

    /// Every entry name in the archive.
    pub(crate) fn part_names(&self) -> &[String] {
        self.zip.names()
    }

    fn find(&self, part: &str) -> Option<String> {
        if let Some(stored) = self.zip.find(part) {
            return Some(stored.to_string());
        }
        // Targets are IRIs: try the percent-decoded form too.
        let decoded = percent_decode(part);
        self.zip.find(&decoded).map(str::to_string)
    }

    /// The declared content type of a part.
    pub(crate) fn content_type(&self, part: &str) -> Option<&str> {
        if let Some(ct) = self.overrides.get(&normalize(part)) {
            return Some(ct);
        }
        let ext = part.rsplit_once('.')?.1.to_ascii_lowercase();
        self.defaults.get(&ext).map(String::as_str)
    }

    /// Read a part's bytes. Missing parts give `None`; parts that cannot
    /// be read (too large, corrupt) are reported in `doc` and give `None`.
    pub(crate) fn read(&mut self, part: &str, doc: &mut Document) -> Option<Vec<u8>> {
        let stored = self.find(part)?;
        match self.zip.read(&stored) {
            Ok(data) => data,
            Err(e) => {
                doc.warn(format!("could not read `{stored}`: {e}"));
                None
            }
        }
    }

    /// Read and parse an XML part. Damage is reported in `doc`; whatever
    /// could be parsed is returned.
    pub(crate) fn xml(&mut self, part: &str, doc: &mut Document) -> Option<Element> {
        self.xml_with(part, doc, &[])
    }

    /// [`Package::xml`], keeping all whitespace inside the `mixed` elements.
    pub(crate) fn xml_with(
        &mut self,
        part: &str,
        doc: &mut Document,
        mixed: &[&str],
    ) -> Option<Element> {
        let bytes = self.read(part, doc)?;
        let parsed = xml::parse_with(&bytes, mixed);
        if let Some(err) = parsed.error {
            doc.warn(format!(
                "`{part}` is damaged ({err}); content after that point is lost"
            ));
        }
        Some(parsed.root)
    }

    /// The relationships of a part (`""` for the package itself).
    pub(crate) fn rels(&mut self, part: &str) -> Relationships {
        let key = normalize(part);
        if let Some(rels) = self.rels.get(&key) {
            return rels.clone();
        }
        let rels = self.load_rels(&key);
        self.rels.insert(key, rels.clone());
        rels
    }

    fn load_rels(&mut self, part: &str) -> Relationships {
        let (dir, file) = match part.rsplit_once('/') {
            Some((dir, file)) => (format!("{dir}/"), file),
            None => (String::new(), part),
        };
        let rels_name = format!("{dir}_rels/{file}.rels");
        let Some(stored) = self.find(&rels_name) else {
            return Relationships::default();
        };
        let Ok(Some(bytes)) = self.zip.read(&stored) else {
            return Relationships::default();
        };
        let root = xml::parse(&bytes).root;
        let rels = root
            .children()
            .filter(|el| el.local() == "Relationship")
            .filter_map(|el| {
                let id = el.attr("Id")?.to_string();
                let rel_type = el.attr("Type").unwrap_or_default().to_string();
                let raw = el.attr("Target")?.trim();
                let external = el
                    .attr("TargetMode")
                    .is_some_and(|m| m.eq_ignore_ascii_case("External"));
                let target = if external {
                    raw.to_string()
                } else {
                    resolve_target(&dir, raw)
                };
                Some(Relationship {
                    id,
                    rel_type,
                    target,
                    external,
                })
            })
            .collect();
        Relationships { rels }
    }

    /// The main part (`word/document.xml`, `xl/workbook.xml`, ...).
    pub(crate) fn main_part(&mut self) -> Result<String> {
        let rels = self.rels("");
        if let Some(target) = rels
            .first_target("officeDocument")
            .or_else(|| rels.first_target("document"))
        {
            return Ok(target.to_string());
        }
        // Damaged packages without root relationships: fall back to the
        // part whose content type says it is the main document.
        self.overrides
            .iter()
            .find(|(_, ct)| ct.ends_with(".main+xml"))
            .map(|(name, _)| name.clone())
            .ok_or_else(|| {
                Error::malformed(self.format.name, "the package has no main document part")
            })
    }
}

fn normalize(part: &str) -> String {
    part.trim_start_matches('/').to_lowercase()
}

/// Resolve a relationship target against the directory of its source part.
pub(crate) fn resolve_target(dir: &str, target: &str) -> String {
    let target = target
        .split('#')
        .next()
        .unwrap_or(target)
        .replace('\\', "/");
    let joined = if let Some(absolute) = target.strip_prefix('/') {
        absolute.to_string()
    } else {
        format!("{dir}{target}")
    };
    let mut segments: Vec<&str> = Vec::new();
    for segment in joined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    segments.join("/")
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_resolve_relative_to_the_source_part() {
        assert_eq!(
            resolve_target("word/", "media/image1.png"),
            "word/media/image1.png"
        );
        assert_eq!(
            resolve_target("ppt/slides/", "../media/a.png"),
            "ppt/media/a.png"
        );
        assert_eq!(
            resolve_target("xl/", "/xl/worksheets/sheet1.xml"),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(resolve_target("", "word/document.xml"), "word/document.xml");
        assert_eq!(resolve_target("a/", "../../../x.xml"), "x.xml");
    }

    #[test]
    fn percent_escapes_are_decoded() {
        assert_eq!(percent_decode("media/image%201.png"), "media/image 1.png");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%e2%82%ac"), "\u{20ac}");
    }
}
