//! State shared by the Office Open XML readers while they read one package:
//! the package, the document being built, and helpers that turn
//! relationships into parts.

use doctopus_core::sniff::mime_for_extension;
use doctopus_core::{Document, Error, Input, Part, PartId, PartKind, ReadOptions, Result};

use super::package::{Package, Relationships};
use super::props;
use super::xml::Element;
use crate::ole::object;

/// One package being read into one document.
#[derive(Debug)]
pub(crate) struct Context<'a> {
    pub(crate) pkg: Package<'a>,
    pub(crate) doc: Document,
    pub(crate) options: &'a ReadOptions,
}

impl<'a> Context<'a> {
    /// Open the package behind `input` and read its document properties.
    pub(crate) fn open(input: &Input<'a>) -> Result<Self> {
        if input.data.starts_with(&CFB_MAGIC) {
            // Encrypted packages are wrapped in a compound file.
            return Err(Error::encrypted(encrypted_format_name(input)));
        }
        let mut pkg = Package::open(input.data, input.format, input.options.max_part_size)?;
        let mut doc = Document::new(input.format);
        if pkg.is_recovered() {
            doc.warn("the file is truncated or damaged (zip directory missing); content was recovered from what is left and may be incomplete");
        }
        props::read(&mut pkg, &mut doc);
        let macros = pkg
            .part_names()
            .iter()
            .any(|n| n.to_ascii_lowercase().ends_with("vbaproject.bin"));
        if macros {
            doc.metadata.set("macros", "yes (VBA project)");
        }
        Ok(Self {
            pkg,
            doc,
            options: input.options,
        })
    }

    /// The main part's name and content.
    pub(crate) fn main_xml(&mut self) -> Result<(String, Element)> {
        let main = self.pkg.main_part()?;
        match self.xml(&main) {
            Some(root) => Ok((main, root)),
            None => {
                let why = if self.pkg.is_recovered() {
                    format!("the file is truncated and its main part `{main}` is lost")
                } else {
                    format!("the main part `{main}` is missing")
                };
                Err(Error::malformed(self.doc.format.name, why))
            }
        }
    }

    /// Read and parse an XML part (damage is recorded as a warning).
    pub(crate) fn xml(&mut self, part: &str) -> Option<Element> {
        self.pkg.xml(part, &mut self.doc)
    }

    /// The relationships of a part.
    pub(crate) fn rels(&mut self, part: &str) -> Relationships {
        self.pkg.rels(part)
    }

    /// The target of an external relationship (a hyperlink), or `#anchor`
    /// style targets kept as written.
    pub(crate) fn hyperlink(rels: &Relationships, id: &str) -> Option<String> {
        let rel = rels.get(id)?;
        rel.external
            .then(|| rel.target.trim().to_string())
            .filter(|t| !t.is_empty())
    }

    /// Attach the picture a relationship points to. Linked (external)
    /// pictures become parts without data that record the link.
    pub(crate) fn image(
        &mut self,
        rels: &Relationships,
        id: &str,
        location: &str,
    ) -> Option<PartId> {
        self.binary_part(rels, id, PartKind::Image, location)
    }

    /// Attach the audio or video a relationship points to.
    pub(crate) fn media(
        &mut self,
        rels: &Relationships,
        id: &str,
        location: &str,
    ) -> Option<PartId> {
        self.binary_part(rels, id, PartKind::Media, location)
    }

    fn binary_part(
        &mut self,
        rels: &Relationships,
        id: &str,
        kind: PartKind,
        location: &str,
    ) -> Option<PartId> {
        let rel = rels.get(id)?;
        if rel.external {
            let origin = format!("link:{}", rel.target);
            if let Some(existing) = self.doc.part_by_origin(&origin) {
                return Some(existing.id.clone());
            }
            let name = file_name(&rel.target);
            let mut part = Part::new(kind, Vec::new())
                .with_link(rel.target.clone())
                .with_origin(origin)
                .with_location(location);
            if let Some(mime) = name
                .rsplit_once('.')
                .and_then(|(_, ext)| mime_for_extension(ext))
            {
                part = part.with_content_type(mime);
            }
            let part = part.with_name(name);
            return Some(self.doc.add_part(part));
        }
        let target = rel.target.clone();
        if let Some(existing) = self.doc.part_by_origin(&target) {
            return Some(existing.id.clone());
        }
        let Some(data) = self.pkg.read(&target, &mut self.doc) else {
            self.doc
                .warn(format!("missing {} `{target}`", kind_noun(kind)));
            return None;
        };
        let mut part = Part::new(kind, data)
            .with_name(file_name(&target))
            .with_origin(target.clone())
            .with_location(location);
        if let Some(ct) = self.pkg.content_type(&target) {
            part = part.with_content_type(ct);
        }
        Some(self.doc.add_part(part))
    }

    /// Attach an embedded object or package (`oleObject1.bin`,
    /// `Microsoft_Excel_Worksheet.xlsx`). OLE wrappers are removed, so the
    /// part holds the file the user inserted whenever it can be recovered.
    pub(crate) fn object(
        &mut self,
        rels: &Relationships,
        id: &str,
        prog_id: Option<&str>,
        location: &str,
    ) -> Option<PartId> {
        let rel = rels.get(id)?;
        let prog_id = prog_id.map(str::trim).filter(|p| !p.is_empty());
        if rel.external {
            let origin = format!("link:{}", rel.target);
            if let Some(existing) = self.doc.part_by_origin(&origin) {
                return Some(existing.id.clone());
            }
            let mut description = String::from("linked object");
            if let Some(prog_id) = prog_id {
                description.push_str(&format!(" {prog_id}"));
            }
            let part = Part::new(PartKind::Object, Vec::new())
                .with_name(file_name(&rel.target))
                .with_link(rel.target.clone())
                .with_description(description)
                .with_origin(origin)
                .with_location(location);
            return Some(self.doc.add_part(part));
        }
        let target = rel.target.clone();
        if let Some(existing) = self.doc.part_by_origin(&target) {
            return Some(existing.id.clone());
        }
        let Some(data) = self.pkg.read(&target, &mut self.doc) else {
            self.doc.warn(format!("missing embedded object `{target}`"));
            return None;
        };
        let unwrapped = object::unwrap(data, prog_id, self.options.max_part_size);
        let declared = self.pkg.content_type(&target).map(str::to_string);
        let name = file_name(&target);
        let part = unwrapped.into_part(target, location, Some(name), declared.as_deref());
        Some(self.doc.add_part(part))
    }
}

const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// The format named in an "encrypted" error: the detected one when the file
/// extension agrees, otherwise a generic name (the real type is only known
/// after decryption).
fn encrypted_format_name(input: &Input<'_>) -> &'static str {
    let ext = input
        .name
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase());
    match ext {
        Some(ext) if input.format.extensions.contains(&ext.as_str()) => input.format.name,
        _ => "Office Open XML document",
    }
}

/// The last segment of a part name or path.
pub(crate) fn file_name(target: &str) -> String {
    target
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(target)
        .to_string()
}

fn kind_noun(kind: PartKind) -> &'static str {
    match kind {
        PartKind::Image => "image",
        PartKind::Media => "media file",
        _ => "part",
    }
}
