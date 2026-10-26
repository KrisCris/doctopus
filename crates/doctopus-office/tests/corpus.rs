//! Every file of the test corpus rendered as Markdown and compared with a
//! reviewed snapshot in `tests/snapshots/`.
//!
//! After an intended change in the output, review and accept the new
//! snapshots with `cargo insta review` (from `cargo install cargo-insta`).

#![allow(missing_docs, clippy::print_stderr)]

mod common;

use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;

use doctopus_core::Address;
use doctopus_core::render::{self, RenderOptions};

#[test]
fn corpus_renders_as_reviewed() {
    let Some(files) = common::corpus() else {
        return;
    };
    let engine = common::engine();
    let mut settings = insta::Settings::clone_current();
    settings.set_prepend_module_to_snapshot(false);
    settings.set_omit_expression(true);
    let mut changed = Vec::new();
    for file in &files {
        let output = match engine.open(&file.path) {
            Ok(doc) => {
                let address = Address {
                    file: PathBuf::from(&file.name),
                    parts: Vec::new(),
                };
                let first = render::markdown(&doc, &RenderOptions::new(address.clone())).text;
                let again = render::markdown(&doc, &RenderOptions::new(address)).text;
                assert_eq!(first, again, "{}: output is not deterministic", file.name);
                first
            }
            Err(err) => format!("error: {err}\n"),
        };
        settings.set_input_file(&file.path);
        let snapshot = file.name.replace('/', "__");
        // Keep going after a mismatch so one run shows every change.
        let matched = panic::catch_unwind(AssertUnwindSafe(|| {
            settings.bind(|| insta::assert_snapshot!(snapshot, output));
        }));
        if matched.is_err() {
            changed.push(file.name.clone());
        }
    }
    assert!(
        changed.is_empty(),
        "{} snapshot(s) changed: {changed:?}",
        changed.len()
    );
}
