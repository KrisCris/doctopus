//! Damaged and hostile input. The reader must return (a document or an
//! error) without panicking, in reasonable time and memory.
//!
//! `damaged_files_never_crash_the_reader` mutates each PDF a few times: the
//! PDF files of the corpus, the PDF files embedded in its Office files, and
//! a few documents built here. For a longer fuzzing run:
//!
//! ```sh
//! DOCTOPUS_FUZZ_ROUNDS=300 cargo test --release -p doctopus-pdf --test robustness
//! ```

#![allow(missing_docs, clippy::print_stderr)]

mod common;

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;
use std::time::{Duration, Instant};

use common::Doc;
use doctopus_core::{Input, Probe, ReadOptions};
use hayro_syntax::{Filter, Pdf};

use hayro_syntax::object::{ObjectIdentifier, Stream};

/// Mutated copies of each file read per run.
const DEFAULT_ROUNDS: u64 = 6;
/// The slowest acceptable read of one damaged file, in a debug build.
const MAX_READ_TIME: Duration = Duration::from_secs(20);

thread_local! {
    static FUZZING: Cell<bool> = const { Cell::new(false) };
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// While `FUZZING` is set, record panics instead of printing them; other
/// panics (failed assertions) print as usual.
fn quiet_panics() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let default = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if FUZZING.get() {
                let message = info
                    .payload()
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| info.payload().downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                let location = info
                    .location()
                    .map(|l| l.file().replace('\\', "/") + ":" + &l.line().to_string())
                    .unwrap_or_default();
                LAST_PANIC.set(Some(format!("{message} at {location}")));
            } else {
                default(info);
            }
        }));
    });
}

/// A small deterministic random number generator (xorshift64).
struct Rng(u64);

impl Rng {
    fn new(seed: &str) -> Self {
        let hash = seed.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        });
        Self(hash | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number in `0..n` (`n > 0`).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Flip a few bytes of `data` after `keep`, or cut it short.
fn damage(data: &mut Vec<u8>, keep: usize, rng: &mut Rng) {
    let keep = keep.min(data.len() / 2);
    if data.len() <= keep + 1 {
        return;
    }
    if rng.below(4) == 0 {
        let cut = keep + rng.below(data.len() - keep);
        data.truncate(cut);
    } else {
        for _ in 0..=rng.below(16) {
            let at = keep + rng.below(data.len() - keep);
            data[at] = rng.next() as u8;
        }
    }
}

/// A damaged copy of a file. Most rounds damage the decoded data of one
/// stream (a page's content, a font, a picture) and write it back as an
/// incremental update, which reaches the content parsers far more often
/// than damaging the file itself.
fn mutate(original: &[u8], rng: &mut Rng) -> Vec<u8> {
    if rng.below(3) > 0
        && let Some(data) = mutate_stream(original, rng)
    {
        return data;
    }
    let mut data = original.to_vec();
    damage(&mut data, 16, rng);
    data
}

/// Damage one stream and append a new version of it, with a cross-reference
/// section and a trailer pointing back to the original ones.
fn mutate_stream(original: &[u8], rng: &mut Rng) -> Option<Vec<u8>> {
    // Encrypted streams would need encrypting again.
    if original.windows(8).any(|w| w == b"/Encrypt") {
        return None;
    }
    let pdf = Pdf::new(original.to_vec()).ok()?;
    let streams: Vec<(ObjectIdentifier, Stream<'_>)> = object_ids(original)
        .into_iter()
        .filter_map(|id| Some((id, pdf.xref().get::<Stream<'_>>(id)?)))
        .filter(|(_, stream)| {
            let dict = stream.dict();
            !dict.contains_key(b"DecodeParms")
                && !dict.contains_key(b"DP")
                && matches!(stream.filters().as_slice(), [] | [Filter::FlateDecode])
        })
        .collect();
    if streams.is_empty() {
        return None;
    }
    let (id, stream) = &streams[rng.below(streams.len())];
    let mut data = stream.decoded().ok()?.to_vec();
    damage(&mut data, 0, rng);
    if !stream.filters().is_empty() {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&data).ok()?;
        data = encoder.finish().ok()?;
    }
    // The old /Length stays; readers find the end of the data by its
    // `endstream`.
    let (number, generation) = numbers(*id)?;
    let previous = last_startxref(original)?;
    let root = numbers(pdf.xref().root_id())?;
    let mut out = original.to_vec();
    out.push(b'\n');
    let offset = out.len();
    out.extend_from_slice(format!("{number} {generation} obj\n").as_bytes());
    out.extend_from_slice(stream.dict().data());
    out.extend_from_slice(b"\nstream\n");
    out.extend_from_slice(&data);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    let xref = out.len();
    out.extend_from_slice(
        format!(
            "xref\n0 1\n0000000000 65535 f \n{number} 1\n{offset:010} {generation:05} n \n\
             trailer\n<< /Size {} /Root {} {} R /Prev {previous} >>\nstartxref\n{xref}\n%%EOF\n",
            number + 1_000_000,
            root.0,
            root.1,
        )
        .as_bytes(),
    );
    Some(out)
}

/// The identifiers of the objects written out in the file, `N G obj`.
fn object_ids(data: &[u8]) -> Vec<ObjectIdentifier> {
    let mut ids = Vec::new();
    for (at, _) in data.windows(4).enumerate().filter(|(_, w)| *w == b" obj") {
        let head = &data[at.saturating_sub(24)..at];
        let text = String::from_utf8_lossy(head);
        let mut words = text.split_ascii_whitespace().rev();
        if let (Some(generation), Some(number)) = (words.next(), words.next())
            && let (Ok(generation), Ok(number)) = (generation.parse(), number.parse())
        {
            ids.push(ObjectIdentifier::new(number, generation));
        }
    }
    ids.sort_unstable_by_key(|id| format!("{id:?}"));
    ids.dedup();
    ids
}

fn numbers(id: ObjectIdentifier) -> Option<(i32, i32)> {
    // ObjectIdentifier keeps its numbers private; its debug form shows them.
    let text = format!("{id:?}");
    let digits: Vec<i32> = text
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .filter_map(|s| s.parse().ok())
        .collect();
    match digits[..] {
        [number, generation] => Some((number, generation)),
        _ => None,
    }
}

fn last_startxref(data: &[u8]) -> Option<usize> {
    let at = data.windows(9).rposition(|w| w == b"startxref")?;
    let tail = String::from_utf8_lossy(&data[at + 9..data.len().min(at + 40)]);
    tail.split_ascii_whitespace().next()?.parse().ok()
}

/// Documents built here, so that the test runs without the corpus too:
/// text in several fonts, a form picture with its own fonts, a Type 3
/// font, pictures, links, bookmarks, form fields and an attachment.
fn built() -> Vec<(String, Vec<u8>)> {
    let mut doc = Doc::new(&[
        "BT /F2 20 Tf 72 720 Td (A heading) Tj ET \
         BT /F1 11 Tf 13 TL 72 690 Td (A paragraph with ) Tj /F2 11 Tf (bold) Tj /F1 11 Tf ( text,) Tj \
         0 -13 Td [(kerned) -250 (words)] TJ T* (and a list:) Tj ET \
         BT /F1 11 Tf 90 640 Td (1. first) Tj 0 -13 Td (2. second) Tj ET \
         BT /TT 9 Tf 0 1 -1 0 560 100 Tm (Turned text in a TrueType font.) Tj ET \
         q 200 0 0 100 72 400 cm /Gray Do Q /Form Do \
         BT /T3 12 Tf 72 300 Td (abab) Tj ET",
        "BT /F1 11 Tf 72 720 Td (Second page, with a ) Tj (link.) Tj ET \
         q 50 0 0 25 72 600 cm /Photo Do Q",
    ]);
    let true_type = doc
        .pdf
        .add("<< /Type /Font /Subtype /TrueType /BaseFont /Arial,BoldItalic /FirstChar 32 /LastChar 126 >>");
    let inner_font = doc
        .pdf
        .add("<< /Type /Font /Subtype /Type1 /BaseFont /Times-Italic >>");
    let form = doc.pdf.deflated(
        &format!(
            "/Type /XObject /Subtype /Form /BBox [0 0 612 792] \
             /Resources << /Font << /FI {inner_font} 0 R >> >>"
        ),
        b"BT /FI 10 Tf 72 360 Td (Text in a form picture.) Tj ET",
    );
    let square = doc.pdf.stream("", b"500 0 0 0 500 500 d1 0 0 500 500 re f");
    let to_unicode = doc.pdf.stream(
        "",
        b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
          1 begincodespacerange <00> <FF> endcodespacerange \
          2 beginbfchar <61> <0061> <62> <0062> endbfchar \
          endcmap CMapName currentdict /CMap defineresource pop end end",
    );
    let type3 = doc.pdf.add(format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 500 500] /FontMatrix [0.001 0 0 0.001 0 0] \
         /CharProcs << /square {square} 0 R >> /Encoding << /Differences [97 /square /square] >> \
         /FirstChar 97 /LastChar 98 /Widths [500 500] /ToUnicode {to_unicode} 0 R >>"
    ));
    let pixels: Vec<u8> = (0..48 * 24 * 3).map(|i| (i * 7 % 256) as u8).collect();
    let gray = doc.pdf.deflated(
        "/Type /XObject /Subtype /Image /Width 48 /Height 24 /ColorSpace /DeviceRGB \
         /BitsPerComponent 8",
        &pixels,
    );
    let photo = doc.pdf.stream(
        "/Type /XObject /Subtype /Image /Width 32 /Height 16 /ColorSpace /DeviceGray \
         /BitsPerComponent 8 /Filter /DCTDecode",
        &[
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x10, 0x00, 0x20, 0x01, 0x01, 0x11,
            0x00, 0xFF, 0xD9,
        ],
    );
    doc.fonts[0] = format!("/TT {true_type} 0 R /T3 {type3} 0 R");
    doc.resources[0] = format!("/XObject << /Gray {gray} 0 R /Form {form} 0 R >>");
    doc.resources[1] = format!("/XObject << /Photo {photo} 0 R >>");
    let first = doc.pages[0];
    let attached = doc
        .pdf
        .deflated("/Type /EmbeddedFile", &common::one_page(""));
    doc.extra[1] = format!(
        "/Annots [<< /Subtype /Link /Rect [174 715 210 730] /Dest [{first} 0 R /Fit] >> \
         << /Subtype /FileAttachment /Rect [300 700 320 720] \
         /FS << /F (inner.pdf) /EF << /F {attached} 0 R >> >> >>]"
    );
    let outline = doc.pdf.reserve();
    let item = doc.pdf.add(format!(
        "<< /Title (Start) /Parent {outline} 0 R /Dest [{first} 0 R /Fit] >>"
    ));
    doc.pdf.set(
        outline,
        format!("<< /First {item} 0 R /Last {item} 0 R /Count 1 >>"),
    );
    let field = doc.pdf.add("<< /FT /Tx /T (name) /V (Ada) >>");
    let rich = doc.finish(
        &format!(
            "/Outlines {outline} 0 R /AcroForm << /Fields [{field} 0 R] >> \
             /PageLabels << /Nums [0 << /S /R >>] >>"
        ),
        "/Info << /Title (Built) /CreationDate (D:20240102030405Z) >>",
    );
    vec![
        ("built/rich.pdf".into(), rich),
        (
            "built/plain.pdf".into(),
            common::one_page("BT /F1 12 Tf 72 700 Td (Only text.) Tj ET"),
        ),
    ]
}

#[test]
fn damaged_files_never_crash_the_reader() {
    quiet_panics();
    let rounds = std::env::var("DOCTOPUS_FUZZ_ROUNDS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(DEFAULT_ROUNDS);
    let mut files = built();
    for file in common::corpus().unwrap_or_default() {
        let data = std::fs::read(&file.path).expect("corpus file");
        files.push((file.name, data));
    }
    files.extend(common::embedded_pdfs());
    let registry = common::registry();
    let options = ReadOptions::default();
    let mut crashes = Vec::new();
    let mut slowest = (Duration::ZERO, String::new());
    for (name, original) in &files {
        let (reader, detection) = registry
            .detect(&Probe::new(original, Some(name)))
            .expect("a PDF");
        let mut rng = Rng::new(name);
        for round in 0..rounds {
            let data = mutate(original, &mut rng);
            let input = Input {
                data: &data,
                name: Some(name),
                format: detection.format,
                options: &options,
            };
            let started = Instant::now();
            FUZZING.set(true);
            let result = panic::catch_unwind(AssertUnwindSafe(|| reader.read(&input)));
            FUZZING.set(false);
            let elapsed = started.elapsed();
            if elapsed > slowest.0 {
                slowest = (elapsed, format!("{name} round {round}"));
            }
            if result.is_err() {
                let message = LAST_PANIC.take().unwrap_or_default();
                crashes.push(format!("{name} round {round}: {message}"));
            }
        }
    }
    eprintln!(
        "{} files x {rounds} rounds; slowest read {:?} ({})",
        files.len(),
        slowest.0,
        slowest.1
    );
    assert!(
        crashes.is_empty(),
        "the reader panicked:\n{}",
        crashes.join("\n")
    );
    assert!(
        slowest.0 <= MAX_READ_TIME,
        "{} took {:?}",
        slowest.1,
        slowest.0
    );
}

#[test]
fn the_built_documents_read_cleanly() {
    let engine = common::engine();
    for (name, data) in built() {
        let doc = engine.parse(&data, Some(&name)).expect("readable");
        assert!(doc.warnings.is_empty(), "{name}: {:?}", doc.warnings);
    }
}
