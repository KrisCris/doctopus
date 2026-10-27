//! Helpers shared by the integration tests.

#![allow(dead_code, unreachable_pub, clippy::print_stderr)]

use std::path::{Path, PathBuf};

use doctopus_core::{Engine, Registry};

/// A registry with every reader of this crate.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    doctopus_office::register(&mut registry);
    registry
}

/// An engine with every reader of this crate.
pub fn engine() -> Engine {
    Engine::new(registry())
}

/// One file of the third-party test corpus.
#[derive(Debug)]
pub struct CorpusFile {
    /// `source/file`, as listed in `testdata/corpus.tsv`.
    pub name: String,
    pub path: PathBuf,
}

/// The files listed in `testdata/corpus.tsv`, or `None` when the corpus has
/// not been fetched (`scripts/fetch-corpus.sh`). A partly fetched corpus
/// fails the test instead of silently testing less. PDF files are left to
/// the tests of `doctopus-pdf`.
pub fn corpus() -> Option<Vec<CorpusFile>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let manifest = std::fs::read_to_string(root.join("corpus.tsv")).expect("testdata/corpus.tsv");
    let files: Vec<CorpusFile> = manifest
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| {
            let file = line.split('\t').nth(1).unwrap_or_default();
            !file.to_ascii_lowercase().ends_with(".pdf")
        })
        .map(|line| {
            let mut fields = line.split('\t');
            let source = fields.next().expect("source column");
            let file = fields.next().expect("file column");
            CorpusFile {
                name: format!("{source}/{file}"),
                path: root.join("corpus").join(source).join(file),
            }
        })
        .collect();
    if !root.join("corpus").is_dir() {
        eprintln!("skipped: the test corpus is missing; fetch it with scripts/fetch-corpus.sh");
        return None;
    }
    let missing: Vec<&str> = files
        .iter()
        .filter(|f| !f.path.is_file())
        .map(|f| f.name.as_str())
        .collect();
    assert!(
        missing.is_empty(),
        "corpus files missing (run scripts/fetch-corpus.sh): {missing:?}"
    );
    Some(files)
}
