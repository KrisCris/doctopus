//! The document model as JSON.

use serde::Serialize;

use super::RenderOptions;
use crate::Document;

#[derive(Serialize)]
struct Envelope<'a> {
    source: String,
    #[serde(flatten)]
    document: &'a Document,
}

/// Serialize the whole document tree (including parsed embedded documents)
/// as pretty-printed JSON. Binary payloads are not included; extract them
/// separately.
pub fn json(doc: &Document, options: &RenderOptions) -> String {
    let envelope = Envelope {
        source: options.source.to_string(),
        document: doc,
    };
    serde_json::to_string_pretty(&envelope).unwrap_or_else(|e| {
        serde_json::json!({ "error": format!("serialization failed: {e}") }).to_string()
    }) + "\n"
}
