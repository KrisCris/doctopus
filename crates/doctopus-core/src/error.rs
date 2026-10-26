//! Error type shared by the engine and every reader.

/// Convenient alias used throughout doctopus.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong while opening, parsing or addressing a document.
///
/// Readers should prefer [`Error::Malformed`] for damaged input and push
/// recoverable problems into [`Document::warnings`](crate::Document::warnings)
/// instead of failing the whole document.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The file could not be read from disk.
    #[error("cannot read `{path}`: {source}")]
    Io {
        /// Path as given by the caller.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// No registered reader understands the input.
    #[error("unsupported file type{}", describe_detected(detected.as_deref()))]
    Unsupported {
        /// What the content looks like, when it could be identified
        /// (for example `"PDF document"`).
        detected: Option<String>,
    },

    /// The document is encrypted and cannot be read without a password.
    #[error("{format} is encrypted or password-protected")]
    Encrypted {
        /// Human-readable format name.
        format: String,
    },

    /// The input claims to be a known format but is damaged or truncated.
    #[error("malformed {format}: {message}")]
    Malformed {
        /// Human-readable format name.
        format: String,
        /// What is wrong.
        message: String,
    },

    /// A part address (such as `report.docx#emb2`) does not resolve.
    #[error("no part `{id}` in {within}")]
    PartNotFound {
        /// The id that failed to resolve.
        id: String,
        /// The document or part that was searched.
        within: String,
        /// Ids that do exist at that level, to help the caller recover.
        available: Vec<String>,
    },

    /// The caller passed an argument that does not apply to this document.
    #[error("{0}")]
    InvalidArgument(String),
}

impl Error {
    /// Shorthand for [`Error::Malformed`].
    pub fn malformed(format: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Malformed {
            format: format.into(),
            message: message.into(),
        }
    }

    /// Shorthand for [`Error::Encrypted`].
    pub fn encrypted(format: impl Into<String>) -> Self {
        Self::Encrypted {
            format: format.into(),
        }
    }
}

fn describe_detected(detected: Option<&str>) -> String {
    detected.map(|d| format!(" ({d})")).unwrap_or_default()
}
