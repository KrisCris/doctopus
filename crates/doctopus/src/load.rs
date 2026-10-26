//! Opening an address: the file, or a part nested inside it.

use doctopus_core::render::{describe_part, shell_quote};
use doctopus_core::{Address, Document, Engine, Error, Part, ReadOptions, Registry};

use crate::failure::{Code, Failure};

/// An engine that knows every format of this build.
pub(crate) fn engine(max_depth: u32, formulas: bool) -> Engine {
    let mut registry = Registry::new();
    doctopus_office::register(&mut registry);
    let mut options = ReadOptions::default();
    options.max_depth = max_depth;
    options.formulas = formulas;
    Engine::new(registry).with_options(options)
}

/// What an address points at.
#[derive(Debug)]
pub(crate) enum Loaded {
    /// The file itself, and its size in bytes.
    File(Document, u64),
    /// A part inside it, payload included.
    Part(Part),
}

impl Loaded {
    /// The document at the address: the file's, or the part's when
    /// doctopus can read the part.
    pub(crate) fn document(&self) -> Option<&Document> {
        match self {
            Self::File(document, _) => Some(document),
            Self::Part(part) => part.document.as_deref(),
        }
    }

    /// The addressed part, if the address names one.
    pub(crate) fn part(&self) -> Option<&Part> {
        match self {
            Self::File(..) => None,
            Self::Part(part) => Some(part),
        }
    }

    /// The document and the size of the file or part, or a failure
    /// explaining why the part is not a document.
    pub(crate) fn into_document(self, address: &Address) -> Result<(Document, u64), Failure> {
        let part = match self {
            Self::File(document, size) => return Ok((document, size)),
            Self::Part(Part {
                document: Some(document),
                size,
                ..
            }) => return Ok((*document, size)),
            Self::Part(part) => part,
        };
        if part.data.is_empty() {
            let what = match &part.link {
                Some(link) => format!("is linked, not embedded: {link}"),
                None => "is empty".into(),
            };
            return Err(Failure::new(Code::Usage, format!("`{address}` {what}")));
        }
        let failure = match &part.error {
            Some(err) => Failure::new(
                Code::Failed,
                format!("`{address}` could not be read: {err}"),
            ),
            None => Failure::new(
                Code::Usage,
                format!(
                    "`{address}` is not a document doctopus can read: {}",
                    describe_part(&part)
                ),
            ),
        };
        Err(failure.hint(format!(
            "save it to a file with: doctopus extract {} -o <dir>",
            shell_quote(&address.to_string())
        )))
    }
}

/// Read the file an address names and find the addressed part in it.
pub(crate) fn load(engine: &Engine, address: &Address) -> Result<Loaded, Failure> {
    let data = std::fs::read(&address.file).map_err(|source| Error::Io {
        path: address.file.display().to_string(),
        source,
    })?;
    let name = address.file.file_name().and_then(|n| n.to_str());
    let document = engine.parse(&data, name)?;
    if address.parts.is_empty() {
        return Ok(Loaded::File(document, data.len() as u64));
    }
    document
        .into_part(&address.part_path())
        .map(Loaded::Part)
        .map_err(|err| part_not_found(err, address))
}

fn part_not_found(err: Error, address: &Address) -> Failure {
    let file = shell_quote(&address.file.display().to_string());
    Failure::from(err).hint(format!("list every part with: doctopus info {file}"))
}
