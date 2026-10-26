//! Render an Office file as Markdown using only the library crates.
//!
//! ```sh
//! cargo run -p doctopus-office --example render -- report.docx
//! ```

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;

use doctopus_core::render::{RenderOptions, markdown};
use doctopus_core::{Address, Engine, Registry};

fn main() -> ExitCode {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: render <file>");
        return ExitCode::FAILURE;
    };
    let mut registry = Registry::new();
    doctopus_office::register(&mut registry);
    let engine = Engine::new(registry);
    match engine.open(&path) {
        Ok(doc) => {
            let options = RenderOptions::new(Address::parse(&path));
            print!("{}", markdown(&doc, &options).text);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
