//! The file as one tree. Streams that list pointers contain the streams
//! they point to; streams of chunks contain their chunks, nested by the
//! level each chunk records. Either way a page holds its shapes and a shape
//! its properties, so the reader works the same on both layouts (and on
//! all three versions).

use std::collections::HashSet;
use std::ops::Range;
use std::rc::Rc;

use super::stream::{Chunk, Layout, NO_ID, Pointer, Streams, V5_LISTS, Version, records_v5};
use crate::binary::u32_at;

/// Pointer nesting followed at most.
const MAX_DEPTH: usize = 24;
/// Chunk nesting kept at most; deeper chunks become siblings.
const MAX_NESTING: usize = 16;
/// Nodes built at most; a damaged pointer tree can reach the same streams
/// over and over.
const MAX_NODES: usize = 1_000_000;

/// The colors stream (its pointer format claims pointers it does not have).
const COLORS: u32 = 0x16;

/// A stream or chunk.
#[derive(Debug)]
pub(super) struct Node {
    pub(super) kind: u32,
    /// The chunk id, or the stream's index in the stream that points to it.
    pub(super) id: u32,
    /// The pointer format, for a stream.
    pub(super) format: Option<u16>,
    buffer: Rc<[u8]>,
    data: Range<usize>,
    extended: Range<usize>,
    pub(super) children: Vec<Node>,
}

impl Node {
    pub(super) fn data(&self) -> &[u8] {
        self.buffer.get(self.data.clone()).unwrap_or_default()
    }

    /// The data and the trailer after it.
    pub(super) fn extended(&self) -> &[u8] {
        self.buffer.get(self.extended.clone()).unwrap_or_default()
    }

    /// The children of a list chunk in the order the list gives: after a
    /// sub-header (its length first, then the list's), the ids of the
    /// members. Members the list leaves out follow in file order.
    pub(super) fn ordered_children(&self) -> Vec<&Node> {
        let data = self.extended();
        let listed: Vec<u32> = match (u32_at(data, 0), u32_at(data, 4)) {
            (Some(header), Some(length)) => {
                let start = 8usize.saturating_add(header as usize);
                let end = start.saturating_add(length as usize).min(data.len());
                data.get(start..end)
                    .unwrap_or_default()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| u32::from_le_bytes(*c))
                    .collect()
            }
            _ => Vec::new(),
        };
        let mut out: Vec<&Node> = listed
            .iter()
            .filter_map(|id| self.children.iter().find(|c| c.id == *id))
            .collect();
        for child in &self.children {
            if !out.iter().any(|c| std::ptr::eq(*c, child)) {
                out.push(child);
            }
        }
        out
    }

    pub(super) fn child(&self, kind: u32) -> Option<&Node> {
        self.children.iter().find(|n| n.kind == kind)
    }

    pub(super) fn children_of(&self, kind: u32) -> impl Iterator<Item = &Node> {
        self.children.iter().filter(move |n| n.kind == kind)
    }

    /// The first node of `kind` below this one, not looking inside nodes
    /// for which `stop` holds.
    pub(super) fn find(&self, kind: u32, stop: &impl Fn(&Node) -> bool) -> Option<&Node> {
        for child in &self.children {
            if child.kind == kind {
                return Some(child);
            }
            if !stop(child)
                && let Some(found) = child.find(kind, stop)
            {
                return Some(found);
            }
        }
        None
    }

    /// The nodes for which `pick` holds, in document order, not looking
    /// inside them or inside nodes for which `stop` holds.
    pub(super) fn collect<'n>(
        &'n self,
        pick: &impl Fn(&Node) -> bool,
        stop: &impl Fn(&Node) -> bool,
        out: &mut Vec<&'n Node>,
    ) {
        for child in &self.children {
            if pick(child) {
                out.push(child);
            } else if !stop(child) {
                child.collect(pick, stop, out);
            }
        }
    }

    fn chunk(buffer: &Rc<[u8]>, chunk: Chunk) -> Node {
        Node {
            kind: chunk.kind,
            id: chunk.id,
            format: None,
            buffer: buffer.clone(),
            data: chunk.data,
            extended: chunk.extended,
            children: Vec::new(),
        }
    }
}

/// The tree under the trailer stream, and whether it had to be cut short.
pub(super) fn read(streams: &mut Streams<'_>, trailer: &Pointer) -> Option<(Node, bool)> {
    let mut builder = Builder {
        streams,
        open: HashSet::new(),
        nodes: 0,
    };
    let root = builder.stream(trailer, 0, 0)?;
    Some((root, builder.nodes > MAX_NODES))
}

struct Builder<'s, 'd> {
    streams: &'s mut Streams<'d>,
    /// Streams being read, to break cycles.
    open: HashSet<Pointer>,
    nodes: usize,
}

impl Builder<'_, '_> {
    fn stream(&mut self, ptr: &Pointer, index: u32, depth: usize) -> Option<Node> {
        if depth > MAX_DEPTH || self.nodes > MAX_NODES {
            return None;
        }
        let buffer = self.streams.data(ptr)?;
        let whole = 0..buffer.len();
        let mut node = match ptr.layout() {
            Layout::Chunks => {
                let chunks = self.streams.chunks(&buffer);
                self.nodes += chunks.len();
                let mut top = nest(&buffer, chunks);
                // The first chunk normally stands for the stream itself.
                if top.first().is_some_and(|n| n.kind == ptr.kind) {
                    let mut first = top.remove(0);
                    first.children.append(&mut top);
                    first
                } else {
                    Node {
                        kind: ptr.kind,
                        id: NO_ID,
                        format: None,
                        buffer: buffer.clone(),
                        data: 0..0,
                        extended: 0..0,
                        children: top,
                    }
                }
            }
            Layout::Blob | Layout::Pointers | Layout::Unknown => {
                let mut node = Node {
                    kind: ptr.kind,
                    id: NO_ID,
                    format: None,
                    buffer: buffer.clone(),
                    data: whole.clone(),
                    extended: whole.clone(),
                    children: Vec::new(),
                };
                if self.streams.version == Version::V5 && V5_LISTS.contains(&ptr.kind) {
                    let records = records_v5(&buffer, whole, 1);
                    node.children = records
                        .into_iter()
                        .map(|c| Node::chunk(&buffer, c))
                        .collect();
                }
                if ptr.layout() == Layout::Pointers && ptr.kind != COLORS && self.open.insert(*ptr)
                {
                    for (i, child) in self.streams.pointers(&buffer, ptr.kind) {
                        if let Some(child) = self.stream(&child, i, depth + 1) {
                            node.children.push(child);
                        }
                    }
                    self.open.remove(ptr);
                }
                node
            }
        };
        self.nodes += 1;
        if node.id == NO_ID {
            node.id = index;
        }
        node.format = Some(ptr.format);
        Some(node)
    }
}

/// Nest chunks by level: a chunk belongs to the closest chunk before it
/// with a lower level.
fn nest(buffer: &Rc<[u8]>, chunks: Vec<Chunk>) -> Vec<Node> {
    fn attach(stack: &mut [(u16, Node)], roots: &mut Vec<Node>, node: Node) {
        match stack.last_mut() {
            Some((_, parent)) => parent.children.push(node),
            None => roots.push(node),
        }
    }
    let mut roots = Vec::new();
    let mut stack: Vec<(u16, Node)> = Vec::new();
    for chunk in chunks {
        let level = chunk.level;
        while stack
            .last()
            .is_some_and(|(open, _)| *open >= level || stack.len() >= MAX_NESTING)
        {
            let (_, done) = stack.pop().expect("checked above");
            attach(&mut stack, &mut roots, done);
        }
        stack.push((level, Node::chunk(buffer, chunk)));
    }
    while let Some((_, done)) = stack.pop() {
        attach(&mut stack, &mut roots, done);
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(kind: u32, level: u16) -> Chunk {
        Chunk {
            kind,
            id: 0,
            level,
            data: 0..0,
            extended: 0..0,
        }
    }

    #[test]
    fn chunks_nest_by_level() {
        let buffer: Rc<[u8]> = Rc::from(Vec::new());
        let roots = nest(
            &buffer,
            vec![
                chunk(0x15, 1),
                chunk(0x46, 2),
                chunk(0x68, 3),
                chunk(0x48, 2),
                chunk(0x0E, 3),
                chunk(0x99, 1),
            ],
        );
        assert_eq!(roots.len(), 2);
        let page = &roots[0];
        let kinds: Vec<u32> = page.children.iter().map(|n| n.kind).collect();
        assert_eq!(kinds, [0x46, 0x48]);
        assert_eq!(page.children[0].children[0].kind, 0x68);
        assert_eq!(page.children[1].children[0].kind, 0x0E);
    }
}
