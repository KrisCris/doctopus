//! Parsing options and safety limits.

/// Settings that influence how documents are parsed.
///
/// Construct with [`ReadOptions::default`] and adjust fields; new fields may
/// be added in minor releases.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ReadOptions {
    /// How many levels of embedded documents to parse. `0` keeps embedded
    /// payloads as opaque parts.
    pub max_depth: u32,
    /// Upper bound on the number of embedded documents parsed in one run,
    /// whatever their depth.
    pub max_documents: u32,
    /// Largest stream or archive entry, in bytes, that will be decompressed.
    pub max_part_size: u64,
    /// Read spreadsheet formulas in addition to cached values.
    pub formulas: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            max_depth: 8,
            max_documents: 500,
            max_part_size: 512 << 20,
            formulas: false,
        }
    }
}
