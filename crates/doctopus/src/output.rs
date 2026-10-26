//! Writing results, notes and crash reports.

use std::cell::RefCell;
use std::io::{self, Write as _};
use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;

use crate::failure::{Code, Failure};

/// Write to stdout. A reader that stops early (`| head`) is not an error.
pub(crate) fn print(text: &str) -> Result<(), Failure> {
    let mut out = io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(Failure::new(
            Code::Failed,
            format!("cannot write output: {err}"),
        )),
    }
}

/// Write a note on stderr, where it does not mix with the document text.
pub(crate) fn note(text: &str) {
    let mut err = io::stderr().lock();
    let _ = err.write_all(text.as_bytes());
    if !text.ends_with('\n') {
        let _ = err.write_all(b"\n");
    }
}

thread_local! {
    /// The last panic, recorded by the hook instead of printed.
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Run the program, turning a panic into a short error report.
///
/// The engine already contains panics inside readers (a damaged embedded
/// object becomes a warning), so the hook stays silent and the message is
/// only shown when a panic actually ends the program.
pub(crate) fn catch_panics(run: impl FnOnce() -> ExitCode) -> ExitCode {
    panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("unknown panic");
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        LAST_PANIC.with(|last| *last.borrow_mut() = Some(format!("{message}{location}")));
    }));
    match panic::catch_unwind(AssertUnwindSafe(run)) {
        Ok(code) => code,
        Err(_) => {
            let message = LAST_PANIC
                .with(|last| last.borrow_mut().take())
                .unwrap_or_else(|| "unknown panic".into());
            let repository = env!("CARGO_PKG_REPOSITORY");
            let report = if repository.is_empty() {
                "this is a bug in doctopus; please report it with the file that triggered it".into()
            } else {
                format!(
                    "this is a bug in doctopus; please report it with the file that triggered it at {repository}/issues"
                )
            };
            Failure::new(Code::Failed, format!("doctopus crashed: {message}"))
                .hint(report)
                .report()
        }
    }
}
