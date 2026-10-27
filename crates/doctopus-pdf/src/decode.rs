//! Bounded stream decoding.
//!
//! The PDF parser decodes streams in full and has no size limit, so a few
//! kilobytes of deflate data could ask for gigabytes. Before anything is
//! decoded, the size it decodes to is measured here: compressed data is
//! inflated into a counter that stops at the limit, and filters that are
//! not run here are assumed to grow data by their worst-case ratio.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io::Read;

use hayro_syntax::Filter;
use hayro_syntax::Pdf;
use hayro_syntax::object::{Array, Dict, Name, Object, Stream};

/// How much a filter can grow its input, for filters not decoded here.
fn worst_ratio(filter: &Filter) -> u64 {
    match filter {
        Filter::AsciiHexDecode | Filter::Ascii85Decode | Filter::Crypt => 1,
        Filter::RunLengthDecode => 128,
        Filter::FlateDecode => 1032,
        _ => 4096,
    }
}

/// The size `stream` decodes to, or a number above `limit` when it would
/// exceed it.
pub(crate) fn decoded_len(stream: &Stream<'_>, limit: u64) -> u64 {
    let raw = stream.raw_data();
    let filters = stream.filters();
    let early_change = stream
        .dict()
        .get::<Dict<'_>>(b"DecodeParms")
        .and_then(|p| p.get::<i64>(b"EarlyChange"))
        != Some(0);
    // Decode while that is cheap and does not grow the data, then count
    // the output of the first compression filter.
    let mut data: Option<Vec<u8>> = None;
    let mut len = raw.len() as u64;
    let mut filters = filters.iter();
    for filter in filters.by_ref() {
        let input = data.as_deref().unwrap_or(&raw[..]);
        match filter {
            Filter::AsciiHexDecode => data = Some(ascii_hex(input)),
            Filter::Ascii85Decode => data = Some(ascii_85(input)),
            Filter::Crypt => continue,
            Filter::FlateDecode => {
                len = inflated_len(input, limit);
                break;
            }
            Filter::LzwDecode => {
                len = lzw_len(input, early_change, limit);
                break;
            }
            Filter::RunLengthDecode => {
                len = run_length_len(input);
                break;
            }
            other => {
                len = (input.len() as u64).saturating_mul(worst_ratio(other));
                break;
            }
        }
        len = data.as_ref().map_or(len, |d| d.len() as u64);
    }
    filters.fold(len, |len, filter| len.saturating_mul(worst_ratio(filter)))
}

/// The decoded data of `stream`, unless it is larger than `limit`.
pub(crate) fn decoded(stream: &Stream<'_>, limit: u64) -> Result<Vec<u8>, Problem> {
    if decoded_len(stream, limit) > limit {
        return Err(Problem::TooLarge);
    }
    stream
        .decoded()
        .map(|data| data.into_owned())
        .map_err(|_| Problem::Damaged)
}

/// Whether [`unwrapped`] can undo every filter of `stream` but the last:
/// ASCII and deflate filters, without a predictor.
pub(crate) fn unwraps(stream: &Stream<'_>) -> bool {
    let filters = stream.filters();
    let Some((_, outer)) = filters.split_last() else {
        return false;
    };
    if outer.is_empty() {
        return true;
    }
    let predicted = |params: &Dict<'_>| params.get::<i64>(b"Predictor").is_some_and(|p| p > 1);
    let dict = stream.dict();
    let predictor = [&b"DecodeParms"[..], b"DP"].into_iter().any(|key| {
        dict.get::<Dict<'_>>(key).is_some_and(|p| predicted(&p))
            || dict.get::<Array<'_>>(key).is_some_and(|a| {
                a.iter::<Object<'_>>()
                    .take(16)
                    .any(|o| matches!(o, Object::Dict(p) if predicted(&p)))
            })
    });
    !predictor
        && outer.iter().all(|f| {
            matches!(
                f,
                Filter::AsciiHexDecode
                    | Filter::Ascii85Decode
                    | Filter::FlateDecode
                    | Filter::Crypt
            )
        })
}

/// The data of `stream` with every filter but the last undone, unless it
/// is larger than `limit`. Some writers wrap JPEG and JPEG 2000 data in
/// deflate or ASCII filters (`[/FlateDecode /DCTDecode]`); this is the
/// JPEG or JPEG 2000 data itself. Only for streams that [`unwraps`]
/// accepts.
pub(crate) fn unwrapped<'a>(stream: &Stream<'a>, limit: u64) -> Result<Cow<'a, [u8]>, Problem> {
    let filters = stream.filters();
    let mut data = stream.raw_data();
    for filter in &filters[..filters.len().saturating_sub(1)] {
        data = Cow::Owned(match filter {
            Filter::AsciiHexDecode => ascii_hex(&data),
            Filter::Ascii85Decode => ascii_85(&data),
            Filter::FlateDecode => inflated(&data, limit)?,
            Filter::Crypt => continue,
            _ => return Err(Problem::Damaged),
        });
    }
    Ok(data)
}

/// Why a stream was not decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Problem {
    TooLarge,
    Damaged,
}

/// Streams the parser decodes by itself while reading pages (content,
/// fonts, patterns) must each stay under `limit`, and all of them
/// together under four times that. Pictures, attached files and metadata
/// are only decoded on request, through [`decoded`], so they are skipped.
pub(crate) fn check(pdf: &Pdf, limit: u64) -> Result<(), String> {
    let budget = limit.saturating_mul(4);
    let mut total = 0u64;
    let mut seen = BTreeSet::new();
    for object in pdf.objects() {
        let Object::Stream(stream) = object else {
            continue;
        };
        let dict = stream.dict();
        let skipped = [
            (&b"Type"[..], &b"EmbeddedFile"[..]),
            (b"Type", b"Metadata"),
            (b"Type", b"XRef"),
            (b"Type", b"ObjStm"),
            (b"Subtype", b"Image"),
        ];
        if skipped
            .iter()
            .any(|(key, value)| dict.get::<Name<'_>>(*key).is_some_and(|n| &*n == *value))
        {
            continue;
        }
        if let Some(id) = dict.obj_id()
            && !seen.insert(format!("{id:?}"))
        {
            continue;
        }
        let len = decoded_len(&stream, limit);
        if len > limit {
            return Err(format!(
                "a stream expands to more than {} MiB; the file may be built to exhaust memory",
                limit >> 20
            ));
        }
        total = total.saturating_add(len);
        if total > budget {
            return Err(format!(
                "its streams expand to more than {} MiB together; the file may be built to exhaust memory",
                budget >> 20
            ));
        }
    }
    Ok(())
}

/// Read from `reader` until it ends, fails, or passes `limit`. Returns the
/// count and whether the data ended cleanly.
fn count(reader: impl Read, limit: u64) -> (u64, bool) {
    let mut reader = reader.take(limit.saturating_add(1));
    let mut buf = [0u8; 16 * 1024];
    let mut total = 0u64;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => return (total, true),
            Ok(n) => total += n as u64,
            Err(_) => return (total, false),
        }
    }
}

/// The inflated size of zlib or raw deflate data, as far as it decodes.
fn inflated_len(data: &[u8], limit: u64) -> u64 {
    let (zlib, clean) = count(flate2::read::ZlibDecoder::new(data), limit);
    if clean || zlib > limit {
        return zlib;
    }
    let (raw, _) = count(flate2::read::DeflateDecoder::new(data), limit);
    zlib.max(raw)
}

/// Inflated zlib or raw deflate data, as far as it decodes, unless it is
/// larger than `limit`. It is measured before anything is kept.
fn inflated(data: &[u8], limit: u64) -> Result<Vec<u8>, Problem> {
    if inflated_len(data, limit) > limit {
        return Err(Problem::TooLarge);
    }
    let mut out = Vec::new();
    if flate2::read::ZlibDecoder::new(data)
        .read_to_end(&mut out)
        .is_err()
    {
        let mut raw = Vec::new();
        let _ = flate2::read::DeflateDecoder::new(data).read_to_end(&mut raw);
        if raw.len() > out.len() {
            out = raw;
        }
    }
    if out.is_empty() {
        return Err(Problem::Damaged);
    }
    Ok(out)
}

/// The decoded size of LZW data. Only the lengths of the table entries are
/// tracked, not their bytes.
fn lzw_len(data: &[u8], early_change: bool, limit: u64) -> u64 {
    const CLEAR: usize = 256;
    const END: usize = 257;
    const FIRST: usize = 258;
    const ENTRIES: usize = 4096;
    let mut lengths = [1u64; ENTRIES];
    let mut next = FIRST;
    let mut width = 9u32;
    let mut previous: Option<usize> = None;
    let mut total = 0u64;
    let mut bits = 0u64;
    let mut held = 0u32;
    let mut bytes = data.iter();
    loop {
        while held < width {
            let Some(&byte) = bytes.next() else {
                return total;
            };
            bits = (bits << 8) | u64::from(byte);
            held += 8;
        }
        held -= width;
        let code = ((bits >> held) & ((1 << width) - 1)) as usize;
        match code {
            CLEAR => {
                next = FIRST;
                width = 9;
                previous = None;
                continue;
            }
            END => return total,
            _ => {}
        }
        let len = match previous {
            _ if code < next => lengths[code],
            Some(p) if code == next => lengths[p] + 1,
            _ => return total,
        };
        if let Some(p) = previous
            && next < ENTRIES
        {
            lengths[next] = lengths[p] + 1;
            next += 1;
        }
        total += len;
        if total > limit {
            return total;
        }
        previous = Some(code);
        if next + usize::from(early_change) >= 1 << width && width < 12 {
            width += 1;
        }
    }
}

/// The decoded size of run-length data.
fn run_length_len(data: &[u8]) -> u64 {
    let mut total = 0u64;
    let mut at = 0;
    while let Some(&n) = data.get(at) {
        match n {
            0..=127 => {
                total += u64::from(n) + 1;
                at += usize::from(n) + 2;
            }
            128 => break,
            _ => {
                total += 257 - u64::from(n);
                at += 2;
            }
        }
    }
    total
}

fn ascii_hex(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2);
    let mut high: Option<u8> = None;
    for &b in data {
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            b'>' => break,
            _ => continue,
        };
        match high.take() {
            Some(h) => out.push(h << 4 | digit),
            None => high = Some(digit),
        }
    }
    if let Some(h) = high {
        out.push(h << 4);
    }
    out
}

fn ascii_85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 5 * 4);
    let mut group = [0u8; 5];
    let mut n = 0;
    let push = |out: &mut Vec<u8>, group: &[u8; 5], n: usize| {
        let value = group.iter().fold(0u64, |acc, &d| acc * 85 + u64::from(d));
        let bytes = (value as u32).to_be_bytes();
        out.extend_from_slice(&bytes[..n - 1]);
    };
    for &b in data.strip_prefix(b"<~").unwrap_or(data) {
        match b {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group[n] = b - b'!';
                n += 1;
                if n == 5 {
                    push(&mut out, &group, 5);
                    n = 0;
                }
            }
            _ => {}
        }
    }
    if n > 1 {
        group[n..].fill(84);
        push(&mut out, &group, n);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn inflating_stops_at_the_limit() {
        let bomb = zlib(&vec![0; 50 << 20]);
        assert!(bomb.len() < 100 << 10);
        assert_eq!(inflated_len(&bomb, 1 << 20), (1 << 20) + 1);
        assert_eq!(inflated_len(&zlib(b"hello"), 1 << 20), 5);
        // Raw deflate without the zlib header, and damaged data.
        let raw = zlib(b"hello hello hello");
        assert_eq!(inflated_len(&raw[2..], 1 << 20), 17);
        assert!(inflated_len(b"\x78\x9cgarbage", 1 << 20) < 16);
    }

    #[test]
    fn lzw_lengths_follow_the_table() {
        // "-----A---B" from the example in PDF 32000-1, 7.4.4.2.
        let data = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(lzw_len(&data, true, 1 << 20), 10);
        assert_eq!(lzw_len(&data, true, 4), 5);
        assert_eq!(lzw_len(&[], true, 10), 0);
    }

    #[test]
    fn ascii_and_run_length_filters_decode() {
        assert_eq!(ascii_hex(b"48 65 6c6C6f>ff"), b"Hello");
        assert_eq!(ascii_hex(b"7"), [0x70]);
        assert_eq!(ascii_85(b"<~87cURDZ~>"), b"Hello");
        assert_eq!(ascii_85(b"z!!~>"), [0, 0, 0, 0, 0]);
        assert_eq!(run_length_len(&[2, b'a', b'b', b'c', 254, b'x', 128, 5]), 6);
        assert_eq!(run_length_len(&[200]), 57);
    }
}
