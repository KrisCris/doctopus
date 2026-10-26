//! The set of readers an [`Engine`](crate::Engine) can dispatch to.

use crate::{Detection, FormatInfo, Probe, Reader};

/// An ordered collection of [`Reader`]s.
///
/// Format crates expose a function that registers their readers, so a
/// front end assembles exactly the formats it wants:
///
/// ```ignore
/// let mut registry = Registry::new();
/// doctopus_office::register(&mut registry);
/// ```
#[derive(Debug, Default)]
pub struct Registry {
    readers: Vec<Box<dyn Reader>>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a reader. When two readers claim an input with the same
    /// confidence, the one registered first wins.
    pub fn register(&mut self, reader: impl Reader + 'static) -> &mut Self {
        self.readers.push(Box::new(reader));
        self
    }

    /// The registered readers, in registration order.
    pub fn readers(&self) -> impl Iterator<Item = &dyn Reader> {
        self.readers.iter().map(|r| r.as_ref())
    }

    /// Every format some reader understands.
    pub fn formats(&self) -> Vec<&'static FormatInfo> {
        self.readers()
            .flat_map(|r| r.formats().iter().copied())
            .collect()
    }

    /// The reader most confident it can parse the probed input.
    pub fn detect(&self, probe: &Probe<'_>) -> Option<(&dyn Reader, Detection)> {
        let mut best: Option<(&dyn Reader, Detection)> = None;
        for reader in self.readers() {
            if let Some(found) = reader.detect(probe)
                && best.is_none_or(|(_, b)| found.confidence > b.confidence)
            {
                best = Some((reader, found));
            }
        }
        best
    }
}
