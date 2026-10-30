//! Errors as the caller sees them: a message, hints on what to do next and
//! an exit code that tells failures apart.

use std::io::{self, Write as _};
use std::process::ExitCode;

use doctopus_core::Error;

/// Hints listing what exists stop after this many names.
const MAX_LISTED: usize = 40;

/// Exit codes; documented in `--help` and the guide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Code {
    /// The file could not be read: missing, damaged, or a bug.
    Failed = 1,
    /// The arguments do not fit the document.
    Usage = 2,
    /// No reader understands the file.
    Unsupported = 3,
    /// The file is encrypted.
    Encrypted = 4,
}

/// A failed command.
#[derive(Debug)]
pub(crate) struct Failure {
    pub(crate) code: Code,
    pub(crate) message: String,
    pub(crate) hints: Vec<String>,
}

impl Failure {
    pub(crate) fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hints: Vec::new(),
        }
    }

    /// Add a hint.
    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }

    /// A failure to write `path`.
    pub(crate) fn write(path: &std::path::Path, err: &io::Error) -> Self {
        Self::new(
            Code::Failed,
            format!("cannot write `{}`: {err}", path.display()),
        )
        .hint("write somewhere else with -o")
    }

    /// Print the failure on stderr and return its exit code.
    pub(crate) fn report(&self) -> ExitCode {
        let mut err = io::stderr().lock();
        let _ = writeln!(err, "error: {}", self.message);
        for hint in &self.hints {
            let _ = writeln!(err, "hint: {hint}");
        }
        ExitCode::from(self.code as u8)
    }
}

impl From<Error> for Failure {
    fn from(err: Error) -> Self {
        let message = err.to_string();
        match err {
            Error::Io { source, .. } => {
                let failure = Self::new(Code::Failed, message);
                match source.kind() {
                    io::ErrorKind::NotFound => failure.hint(
                        "check the path; parts inside a file are addressed as FILE#id (deck.pptx#emb1)",
                    ),
                    _ => failure,
                }
            }
            Error::Unsupported { .. } => Self::new(Code::Unsupported, message).hint(
                "doctopus reads Word, Excel, PowerPoint, Visio and PDF files; `doctopus formats` lists them",
            ),
            Error::Encrypted { .. } => Self::new(Code::Encrypted, message)
                .hint("doctopus cannot decrypt files; an unprotected copy is needed"),
            Error::PartNotFound { available, .. } => {
                let failure = Self::new(Code::Usage, message);
                if available.is_empty() {
                    failure.hint("it contains no parts")
                } else {
                    failure.hint(format!("available: {}", list(&available)))
                }
            }
            Error::InvalidArgument(_) => Self::new(Code::Usage, message),
            Error::Malformed { .. } => Self::new(Code::Failed, message)
                .hint("the file may be damaged, truncated, or misnamed"),
            _ => Self::new(Code::Failed, message),
        }
    }
}

/// `a, b, c` with the tail elided.
pub(crate) fn list(items: &[String]) -> String {
    let mut out = items
        .iter()
        .take(MAX_LISTED)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > MAX_LISTED {
        out.push_str(&format!(" and {} more", items.len() - MAX_LISTED));
    }
    out
}
