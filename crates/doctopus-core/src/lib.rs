//! Core of [doctopus](https://crates.io/crates/doctopus): a format-independent
//! document model, the [`Reader`] plug-in contract, the [`Engine`] that
//! detects formats and recursively parses embedded content, and renderers
//! that produce agent-friendly Markdown, plain text and JSON.
//!
//! This crate knows no file format. Formats live in their own crates (for
//! example `doctopus-office`) that implement [`Reader`] and register
//! themselves in a [`Registry`]:
//!
//! ```ignore
//! use doctopus_core::{Engine, Registry, render};
//!
//! let mut registry = Registry::new();
//! doctopus_office::register(&mut registry);
//! let engine = Engine::new(registry);
//! let doc = engine.open("report.docx")?;
//! let options = render::RenderOptions::new(doctopus_core::Address::parse("report.docx"));
//! println!("{}", render::markdown(&doc, &options).text);
//! ```

mod address;
pub mod container;
mod engine;
mod error;
mod format;
mod model;
mod options;
mod probe;
mod registry;
pub mod render;
mod select;
pub mod sniff;

pub use crate::address::Address;
pub use crate::engine::Engine;
pub use crate::error::{Error, Result};
pub use crate::format::{Confidence, Detection, DocumentKind, FormatInfo, Input, Reader};
pub use crate::model::*;
pub use crate::options::ReadOptions;
pub use crate::probe::Probe;
pub use crate::registry::Registry;
pub use crate::select::{SectionSelector, Selection};
