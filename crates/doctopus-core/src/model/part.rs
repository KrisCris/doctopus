use std::fmt;

use serde::Serialize;

use super::Document;

/// Identifier of a [`Part`] within its document (`img1`, `emb2`).
///
/// Nested parts are addressed by joining ids with `/`: `emb1/img2` is image 2
/// of the document embedded as `emb1`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct PartId(String);

impl PartId {
    /// Wrap an id string. Readers normally let [`Document::add_part`] assign ids.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PartId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a part is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PartKind {
    /// A picture.
    Image,
    /// An embedded document or OLE object (a workbook inside a slide).
    Object,
    /// A file attached as-is (an OLE "package", an attachment).
    File,
    /// Audio or video.
    Media,
    /// Anything else.
    Other,
}

impl PartKind {
    /// Prefix used for generated ids.
    pub fn id_prefix(self) -> &'static str {
        match self {
            Self::Image => "img",
            Self::Object | Self::File => "emb",
            Self::Media => "media",
            Self::Other => "part",
        }
    }
}

/// A binary resource carried by a document: an image, an embedded document,
/// an attached file.
#[derive(Debug, Clone, Serialize)]
pub struct Part {
    /// Id, unique within the owning document. Assigned by [`Document::add_part`].
    pub id: PartId,
    /// What the part is.
    pub kind: PartKind,
    /// Original or suggested file name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Media type, detected from content when the container does not say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Payload size in bytes.
    pub size: u64,
    /// Where the payload lives inside the container
    /// (`word/media/image1.png`, `ObjectPool/_1234`). Also the dedup key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// What the part is, in words (`Microsoft Excel Worksheet`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Where the part first appears (`slide 3`, `Sheet1!B4`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Pixel size, for images whose header could be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<(u32, u32)>,
    /// External target for linked (not embedded) content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// The parsed content, when the payload is itself a readable document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<Box<Document>>,
    /// Why the payload could not be read, when it looked readable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The raw payload.
    #[serde(skip)]
    pub data: Vec<u8>,
}

impl Part {
    /// A part of the given kind holding `data`. Fill in the optional fields
    /// with the builder methods, then pass it to [`Document::add_part`].
    pub fn new(kind: PartKind, data: Vec<u8>) -> Self {
        Self {
            id: PartId::default(),
            kind,
            name: None,
            content_type: None,
            size: data.len() as u64,
            origin: None,
            description: None,
            location: None,
            dimensions: None,
            link: None,
            document: None,
            error: None,
            data,
        }
    }

    /// Set the file name (blank names are ignored).
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = non_blank(name.into());
        self
    }

    /// Set the media type.
    pub fn with_content_type(mut self, content_type: impl Into<String>) -> Self {
        self.content_type = non_blank(content_type.into());
        self
    }

    /// Set the origin inside the container (also the dedup key).
    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        self.origin = non_blank(origin.into());
        self
    }

    /// Set the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = non_blank(description.into());
        self
    }

    /// Set where the part first appears.
    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = non_blank(location.into());
        self
    }

    /// Mark the part as linked to an external target.
    pub fn with_link(mut self, link: impl Into<String>) -> Self {
        self.link = non_blank(link.into());
        self
    }

    /// A file name for extraction: the original name when known, otherwise
    /// the id plus an extension guessed from the content type.
    ///
    /// Container-internal names that hide the real type (`oleObject1.bin`)
    /// get the extension of the detected content type instead.
    pub fn file_name(&self) -> String {
        let known_ext = self
            .content_type
            .as_deref()
            .and_then(crate::sniff::extension_for_mime);
        if let Some(name) = self.name.as_deref() {
            let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
            let safe = sanitize_file_name(base);
            if !safe.is_empty() {
                return match (safe.rsplit_once('.'), known_ext) {
                    (Some((stem, ext)), Some(known))
                        if ext.eq_ignore_ascii_case("bin") && !stem.is_empty() =>
                    {
                        format!("{stem}.{known}")
                    }
                    (None, Some(known)) => format!("{safe}.{known}"),
                    _ => safe,
                };
            }
        }
        format!("{}.{}", self.id, known_ext.unwrap_or("bin"))
    }
}

fn non_blank(s: String) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Make a string safe to use as a file name on every platform.
pub(crate) fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    let mut out: String = trimmed.chars().take(120).collect();
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        out.insert(0, '_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_sanitized() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_file_name("CON.txt"), "_CON.txt");
        assert_eq!(
            sanitize_file_name("Q3 report: final?.docx"),
            "Q3 report_ final_.docx"
        );
    }

    #[test]
    fn file_name_falls_back_to_id_and_mime() {
        let mut part = Part::new(PartKind::Image, vec![1, 2, 3]).with_content_type("image/png");
        part.id = PartId::new("img4");
        assert_eq!(part.file_name(), "img4.png");
        let named = part.clone().with_name("C:\\Users\\me\\Pictures\\logo.png");
        assert_eq!(named.file_name(), "logo.png");
        let mut ole = Part::new(PartKind::Object, vec![]).with_name("oleObject1.bin");
        ole.content_type = Some("application/vnd.ms-excel".into());
        assert_eq!(ole.file_name(), "oleObject1.xls");
    }
}
