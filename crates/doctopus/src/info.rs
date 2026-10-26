//! `doctopus info` and `doctopus formats`.

use std::fmt::Write as _;

use doctopus_core::render::{self, RenderOptions, Summary};
use doctopus_core::{Address, Engine, FormatInfo};
use serde::Serialize;

use crate::cli::{FormatsArgs, InfoArgs};
use crate::failure::Failure;
use crate::load::{engine, load};
use crate::output::print;

/// `doctopus info`.
pub(crate) fn run(args: &InfoArgs, max_depth: u32) -> Result<(), Failure> {
    let engine = engine(max_depth, false);
    let addresses: Vec<Address> = args.addresses.iter().map(|a| Address::parse(a)).collect();
    if let [address] = addresses.as_slice() {
        let out = if args.json {
            to_json(&json(&engine, address)?)
        } else {
            text(&engine, address)?
        };
        return print(&out);
    }
    // Several files: one entry each, failures included, so a survey of a
    // directory is not cut short by one bad file.
    let mut failed = None;
    let mut note = |address: &Address, failure: Failure| {
        failed.get_or_insert(failure.code);
        (address.to_string(), failure.message)
    };
    let out = if args.json {
        #[derive(Serialize)]
        #[serde(untagged)]
        enum Entry {
            Summary(Box<Summary>),
            Failed { source: String, error: String },
        }
        let entries: Vec<Entry> = addresses
            .iter()
            .map(|address| match json(&engine, address) {
                Ok(summary) => Entry::Summary(Box::new(summary)),
                Err(failure) => {
                    let (source, error) = note(address, failure);
                    Entry::Failed { source, error }
                }
            })
            .collect();
        to_json(&entries)
    } else {
        let entries: Vec<String> = addresses
            .iter()
            .map(|address| {
                text(&engine, address).unwrap_or_else(|failure| {
                    let (source, error) = note(address, failure);
                    format!("source: {source}\nerror: {error}\n")
                })
            })
            .collect();
        entries.join("---\n")
    };
    print(&out)?;
    match failed {
        Some(code) => Err(Failure::new(
            code,
            "some files could not be read; see their `error` entries",
        )),
        None => Ok(()),
    }
}

fn text(engine: &Engine, address: &Address) -> Result<String, Failure> {
    let (document, size) = load(engine, address)?.into_document(address)?;
    let options = RenderOptions::new(address.clone());
    Ok(render::info(&document, &options, Some(size)))
}

fn json(engine: &Engine, address: &Address) -> Result<Summary, Failure> {
    let (document, size) = load(engine, address)?.into_document(address)?;
    let options = RenderOptions::new(address.clone());
    Ok(render::summarize(&document, &options, Some(size)))
}

fn to_json(value: &impl Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default() + "\n"
}

/// `doctopus formats`.
pub(crate) fn formats(args: &FormatsArgs) -> Result<(), Failure> {
    #[derive(Serialize)]
    struct Entry {
        #[serde(flatten)]
        format: &'static FormatInfo,
        extensions: &'static [&'static str],
    }
    let engine = engine(0, false);
    let formats = engine.registry().formats();
    if args.json {
        let entries: Vec<Entry> = formats
            .into_iter()
            .map(|format| Entry {
                format,
                extensions: format.extensions,
            })
            .collect();
        return print(&to_json(&entries));
    }
    let width = formats.iter().map(|f| f.name.len()).max().unwrap_or(0);
    let mut out = String::new();
    for format in formats {
        let extensions: Vec<String> = format.extensions.iter().map(|e| format!(".{e}")).collect();
        let _ = writeln!(
            out,
            "{:<5} {:<width$}  {}",
            format.id,
            format.name,
            extensions.join(" ")
        );
    }
    print(&out)
}
