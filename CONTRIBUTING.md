# Contributing to doctopus

Thank you for helping.
[Bug reports](https://github.com/KrisCris/doctopus/issues) with a sample
file are the most useful contribution of all. If you cannot share the
file, the output of `doctopus info FILE` and the error message help too.

## Getting started

You need Rust 1.88 or newer, plus `bash`, `curl` and Python 3 for the
scripts.

```sh
scripts/fetch-corpus.sh   # download the test corpus (about 11 MB, verified by SHA-256)
cargo test --workspace
```

Without the corpus, the corpus tests skip themselves and everything else
still runs.

## Layout

| Path | What it holds |
|---|---|
| `crates/doctopus-core` | The document model, the `Reader` trait, format detection, the engine that parses embedded content, containers (zip and OLE compound files), and the Markdown, text, JSON and info renderers. It knows no file format. |
| `crates/doctopus-office` | Readers for Word, Excel, PowerPoint and Visio: Office Open XML in `src/ooxml`, the binary formats of Office 97-2003 in `src/legacy`. |
| `crates/doctopus-pdf` | The PDF reader, on the hayro interpreter: page text and layout in `src/page.rs` and `src/layout.rs`, the catalog (links, bookmarks, forms, attachments) in `src/catalog.rs`. See [docs/pdf.md](docs/pdf.md). |
| `crates/doctopus` | The command line. |
| `testdata/corpus.tsv` | The third-party test corpus: sources, file names and hashes. The files are downloaded, not committed. |
| `scripts/` | Fetching the corpus, checking release binaries, collecting third-party licenses. |
| `docs/adding-a-format.md` | How to add a reader for a new file type. |
| `docs/pdf.md` | How PDF maps onto the document model, and why the PDF reader uses hayro. |

## Principles

- **The user is often an agent.** Output must be deterministic and
  structured. It is paged rather than unbounded. Every error says what
  exists and what to try next.
- **Untrusted input.** Readers never panic, never trust sizes or counts
  from a file, and read as much as they can. What they cannot read becomes
  a warning. See "Damaged and hostile input" in
  [docs/adding-a-format.md](docs/adding-a-format.md).
- **Self-contained binaries.** No C dependencies and no dynamic libraries
  beyond what the operating system provides. Check new dependencies for
  this, and for permissive licenses (`deny.toml`).

## Checks

CI runs all of these. Run them before opening a pull request:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### Snapshots

`crates/doctopus-office/tests/corpus.rs` renders every Office file of the
corpus and compares the result with the snapshots in
`crates/doctopus-office/tests/snapshots/`. `crates/doctopus-pdf/tests/corpus.rs`
does the same for the PDF files, and for the PDF files embedded in the
Office ones. When a change alters the output on purpose, review the
differences and accept them:

```sh
cargo install cargo-insta
cargo insta test --review -p doctopus-office --test corpus
cargo insta test --review -p doctopus-pdf --test corpus
```

Look at each change. The snapshots are how an output regression gets
noticed.

### Fuzzing

`crates/doctopus-office/tests/robustness.rs` and
`crates/doctopus-pdf/tests/robustness.rs` damage each corpus file a few
times on every test run. After changing a reader, run them longer in
release mode:

```sh
DOCTOPUS_FUZZ_ROUNDS=300 cargo test --release -p doctopus-office --test robustness
DOCTOPUS_FUZZ_ROUNDS=300 cargo test --release -p doctopus-pdf --test robustness
```

A panic in a reader fails the test and names the file, the round and the
source location.

## Releases

1. Update the version in the root `Cargo.toml` and `CHANGELOG.md`.
2. Tag the commit `vX.Y.Z` and push the tag.
3. The release workflow builds and checks binaries for Linux, macOS and
   Windows on x86_64 and arm64. It attaches them to a draft GitHub
   release. Review the draft and publish it.

The release workflow can also be run by hand to build the binaries without
releasing.

## License

Contributions are accepted under the terms of both the MIT license and the
Apache License, Version 2.0, the same as the project.
