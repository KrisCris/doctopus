//! State shared by the 97-2003 readers while they read one compound file:
//! the file, the document being built, and helpers that turn embedded
//! storages and pictures into parts.

use doctopus_core::container::Cfb;
use doctopus_core::sniff::{extension_for_mime, mime_for_extension};
use doctopus_core::{Document, Error, Input, Part, PartId, PartKind, ReadOptions, Result};

use super::officeart::Blip;
use crate::ole::{object, props};

/// One compound file being read into one document.
pub(crate) struct Context<'a> {
    pub(crate) cfb: Cfb<'a>,
    pub(crate) doc: Document,
    pub(crate) options: &'a ReadOptions,
}

impl std::fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("doc", &self.doc)
            .finish_non_exhaustive()
    }
}

impl<'a> Context<'a> {
    /// Open the compound file behind `input` and read its properties.
    pub(crate) fn open(input: &Input<'a>) -> Result<Self> {
        let mut cfb = Cfb::new(input.data, input.options.max_part_size)
            .map_err(|e| Error::malformed(input.format.name, e.to_string()))?;
        let mut doc = Document::new(input.format);
        props::read(&mut cfb, &mut doc);
        Ok(Self {
            cfb,
            doc,
            options: input.options,
        })
    }

    /// Read a stream; failures are recorded as warnings.
    pub(crate) fn stream(&mut self, path: &str) -> Option<Vec<u8>> {
        match self.cfb.read(path) {
            Ok(data) => data,
            Err(e) => {
                self.doc
                    .warn(format!("stream `{path}` could not be read: {e}"));
                None
            }
        }
    }

    /// Read a stream the format cannot do without.
    pub(crate) fn required_stream(&mut self, path: &str) -> Result<Vec<u8>> {
        match self.cfb.read(path) {
            Ok(Some(data)) => Ok(data),
            Ok(None) => Err(Error::malformed(
                self.doc.format.name,
                format!("the `{}` stream is missing", path.trim_start_matches('/')),
            )),
            Err(e) => Err(Error::malformed(self.doc.format.name, e.to_string())),
        }
    }

    /// Record that the file carries a VBA project when any of these
    /// storages exists.
    pub(crate) fn detect_macros(&mut self, storages: &[&str]) {
        if storages.iter().any(|s| self.cfb.is_storage(s)) {
            self.doc.metadata.set("macros", "yes (VBA project)");
        }
    }

    /// Attach the embedded object kept in a storage (`/ObjectPool/_123`,
    /// `/MBD0000ABCD`), unwrapped to the inserted file where possible.
    pub(crate) fn storage_object(
        &mut self,
        path: &str,
        prog_id: Option<&str>,
        location: &str,
    ) -> Option<PartId> {
        let origin = format!("storage:{path}");
        if let Some(existing) = self.doc.part_by_origin(&origin) {
            return Some(existing.id.clone());
        }
        let data = match self.cfb.extract_storage(path) {
            Ok(data) => data,
            Err(e) => {
                self.doc
                    .warn(format!("embedded object `{path}` could not be read: {e}"));
                return None;
            }
        };
        Some(self.object(data, origin, prog_id, location))
    }

    /// Attach an embedded object held in memory (a compound file or a raw
    /// payload).
    pub(crate) fn object(
        &mut self,
        data: Vec<u8>,
        origin: String,
        prog_id: Option<&str>,
        location: &str,
    ) -> PartId {
        if let Some(existing) = self.doc.part_by_origin(&origin) {
            return existing.id.clone();
        }
        let unwrapped = object::unwrap(data, prog_id, self.options.max_part_size);
        let part = unwrapped.into_part(origin, location, None, None);
        self.doc.add_part(part)
    }

    /// Attach a picture; `origin` identifies it so repeated uses share one
    /// part.
    pub(crate) fn picture(&mut self, blip: Blip, origin: String, location: &str) -> PartId {
        if let Some(existing) = self.doc.part_by_origin(&origin) {
            return existing.id.clone();
        }
        let mut part = Part::new(PartKind::Image, blip.data)
            .with_content_type(blip.content_type)
            .with_origin(origin)
            .with_location(location);
        if let Some(name) = blip.name {
            part = part.with_name(picture_name(&name, blip.content_type));
        }
        self.doc.add_part(part)
    }
}

/// The stored name of a picture, with the extension corrected when Office
/// converted the picture to another format (a GIF kept as PNG).
fn picture_name(name: &str, content_type: &str) -> String {
    let Some((stem, ext)) = name.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()) else {
        return name.to_string();
    };
    match (mime_for_extension(ext), extension_for_mime(content_type)) {
        (Some(declared), Some(actual)) if declared != content_type => format!("{stem}.{actual}"),
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converted_pictures_get_their_real_extension() {
        assert_eq!(picture_name("logo.gif", "image/png"), "logo.png");
        assert_eq!(picture_name("photo.JPG", "image/jpeg"), "photo.JPG");
        assert_eq!(picture_name("chart", "image/emf"), "chart");
    }
}
