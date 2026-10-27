//! Every PDF file of the test corpus, and every PDF file embedded in its
//! Office files, rendered as Markdown and compared with a reviewed
//! snapshot in `tests/snapshots/`.
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
    let mut inputs: Vec<(String, Vec<u8>)> = files
        .into_iter()
        .map(|f| {
            let data = std::fs::read(&f.path).expect("corpus file");
            (f.name, data)
        })
        .collect();
    inputs.extend(common::embedded_pdfs());
    let engine = common::engine();
    let mut settings = insta::Settings::clone_current();
    settings.set_prepend_module_to_snapshot(false);
    settings.set_omit_expression(true);
    let mut changed = Vec::new();
    for (name, data) in &inputs {
        let output = match engine.parse(data, Some(name)) {
            Ok(doc) => {
                let address = Address {
                    file: PathBuf::from(name),
                    parts: Vec::new(),
                };
                let first = render::markdown(&doc, &RenderOptions::new(address.clone())).text;
                let again = render::markdown(&doc, &RenderOptions::new(address)).text;
                assert_eq!(first, again, "{name}: output is not deterministic");
                let reread = engine.parse(data, Some(name)).expect("read again");
                assert_eq!(
                    serde_json::to_string(&doc).ok(),
                    serde_json::to_string(&reread).ok(),
                    "{name}: reading is not deterministic"
                );
                first
            }
            Err(err) => format!("error: {err}\n"),
        };
        let snapshot = name.replace('/', "__");
        // Keep going after a mismatch so one run shows every change.
        let matched = panic::catch_unwind(AssertUnwindSafe(|| {
            settings.bind(|| insta::assert_snapshot!(snapshot, output));
        }));
        if matched.is_err() {
            changed.push(name.clone());
        }
    }
    assert!(
        changed.is_empty(),
        "{} snapshot(s) changed: {changed:?}",
        changed.len()
    );
}
