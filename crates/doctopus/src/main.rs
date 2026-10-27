//! `doctopus`: read Word, Excel, PowerPoint, Visio and PDF files, and
//! everything embedded in them, as Markdown, plain text or JSON.
//!
//! The command line is written for coding agents first: output is
//! deterministic and paged, every error says what to try next, and every
//! image or embedded document has an address (`deck.pptx#emb1`) that other
//! commands accept.

mod cli;
mod convert;
mod extract;
mod failure;
mod info;
mod load;
mod output;
mod paths;
mod read;

use std::process::ExitCode;

use clap::Parser as _;

use crate::cli::{Cli, Command};

/// Usage guide printed by `doctopus guide`.
const GUIDE: &str = include_str!("guide.md");

fn main() -> ExitCode {
    output::catch_panics(|| {
        let cli = Cli::parse_from(cli::args());
        let result = match &cli.command {
            Command::Read(args) => read::run(args, cli.max_depth),
            Command::Info(args) => info::run(args, cli.max_depth),
            Command::Extract(args) => extract::run(args, cli.max_depth),
            Command::Convert(args) => convert::run(args, cli.max_depth),
            Command::Formats(args) => info::formats(args),
            Command::Guide => output::print(GUIDE),
        };
        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(failure) => failure.report(),
        }
    })
}
