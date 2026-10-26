//! Damaged and hostile input. Every reader must return (a document or an
//! error) without panicking, in reasonable time and memory.
//!
//! `damaged_files_never_crash_a_reader` mutates each corpus file a few
//! times. For a longer fuzzing run:
//!
//! ```sh
//! DOCTOPUS_FUZZ_ROUNDS=300 cargo test --release -p doctopus-office --test robustness
//! ```

#![allow(missing_docs, clippy::print_stderr)]

mod common;

use std::cell::{Cell, RefCell};
use std::io::{Cursor, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;
use std::time::{Duration, Instant};

use doctopus_core::container::compound_file;
use doctopus_core::render::{self, RenderOptions};
use doctopus_core::{Address, Input, Probe, ReadOptions};

/// Mutated copies of each corpus file read per run.
const DEFAULT_ROUNDS: u64 = 6;
/// The slowest acceptable read of one damaged file, in a debug build.
const MAX_READ_TIME: Duration = Duration::from_secs(20);
/// Dependencies whose panics on damaged input are known. The engine
/// contains them (the document fails with an error); they are reported
/// upstream rather than failing this test.
const CONTAINED: [&str; 1] = ["calamine-"];

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

/// A damaged copy of a file. Most rounds damage one stream of a compound
/// file or one entry of a zip package and repack it, which reaches the
/// format parsers far more often than damaging the container itself.
fn mutate(original: &[u8], rng: &mut Rng) -> Vec<u8> {
    if rng.below(3) > 0 {
        let repacked = if original.starts_with(b"PK") {
            mutate_zip_entry(original, rng)
        } else if original.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
            mutate_stream(original, rng)
        } else {
            None
        };
        if let Some(data) = repacked {
            return data;
        }
    }
    let mut data = original.to_vec();
    damage(&mut data, 512, rng);
    data
}

fn mutate_stream(original: &[u8], rng: &mut Rng) -> Option<Vec<u8>> {
    let file = cfb::CompoundFile::open(Cursor::new(original)).ok()?;
    let entries: Vec<_> = file
        .walk()
        .filter(|e| !e.is_root())
        .map(|e| {
            let path = e.path().to_string_lossy().replace('\\', "/");
            (path, e.is_stream(), *e.clsid())
        })
        .collect();
    let root = *file.root_entry().clsid();
    let mut file = file;
    let streams: Vec<&String> = entries
        .iter()
        .filter(|(_, is_stream, _)| *is_stream)
        .map(|(path, ..)| path)
        .collect();
    if streams.is_empty() {
        return None;
    }
    let target = streams[rng.below(streams.len())].clone();
    let mut out = cfb::CompoundFile::create(Cursor::new(Vec::new())).ok()?;
    out.set_storage_clsid("/", root).ok()?;
    for (path, is_stream, clsid) in &entries {
        if !is_stream {
            out.create_storage(path).ok()?;
            out.set_storage_clsid(path, *clsid).ok()?;
            continue;
        }
        let mut data = Vec::new();
        file.open_stream(path).ok()?.read_to_end(&mut data).ok()?;
        if *path == target {
            damage(&mut data, 0, rng);
        }
        out.create_stream(path).ok()?.write_all(&data).ok()?;
    }
    out.flush().ok()?;
    Some(out.into_inner().into_inner())
}

fn mutate_zip_entry(original: &[u8], rng: &mut Rng) -> Option<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(original)).ok()?;
    if archive.is_empty() {
        return None;
    }
    let target = rng.below(archive.len());
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).ok()?;
        let name = entry.name().ok()?.into_owned();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).ok()?;
        if i == target {
            damage(&mut data, 0, rng);
        }
        writer.start_file(name, stored).ok()?;
        writer.write_all(&data).ok()?;
    }
    Some(writer.finish().ok()?.into_inner())
}

#[test]
fn damaged_files_never_crash_a_reader() {
    let Some(files) = common::corpus() else {
        return;
    };
    quiet_panics();
    let rounds = std::env::var("DOCTOPUS_FUZZ_ROUNDS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(DEFAULT_ROUNDS);
    let registry = common::registry();
    let options = ReadOptions::default();
    let mut crashes = Vec::new();
    let mut contained = 0;
    let mut slowest = (Duration::ZERO, String::new());
    for file in &files {
        let original = std::fs::read(&file.path).expect("corpus file");
        let Some((reader, detection)) = registry.detect(&Probe::new(&original, Some(&file.name)))
        else {
            continue;
        };
        let mut rng = Rng::new(&file.name);
        for round in 0..rounds {
            let data = mutate(&original, &mut rng);
            let input = Input {
                data: &data,
                name: Some(&file.name),
                format: detection.format,
                options: &options,
            };
            let started = Instant::now();
            FUZZING.set(true);
            let result = panic::catch_unwind(AssertUnwindSafe(|| reader.read(&input)));
            FUZZING.set(false);
            let elapsed = started.elapsed();
            if elapsed > slowest.0 {
                slowest = (elapsed, format!("{} round {round}", file.name));
            }
            if result.is_err() {
                let message = LAST_PANIC.take().unwrap_or_default();
                if CONTAINED.iter().any(|dep| message.contains(dep)) {
                    contained += 1;
                } else {
                    crashes.push(format!("{} round {round}: {message}", file.name));
                }
            }
        }
    }
    eprintln!(
        "{} files x {rounds} rounds; slowest read {:?} ({}); {contained} contained dependency panics",
        files.len(),
        slowest.0,
        slowest.1
    );
    assert!(
        crashes.is_empty(),
        "readers panicked:\n{}",
        crashes.join("\n")
    );
    assert!(
        slowest.0 <= MAX_READ_TIME,
        "{} took {:?}",
        slowest.1,
        slowest.0
    );
}

/// Render `data` (named `name`) through the engine, timing the whole read.
fn render_timed(data: &[u8], name: &str) -> (String, Vec<String>, Duration) {
    let started = Instant::now();
    let doc = common::engine().parse(data, Some(name)).expect("readable");
    let address = Address::parse_with(name, |_| true);
    let text = render::markdown(&doc, &RenderOptions::new(address)).text;
    (text, doc.warnings, started.elapsed())
}

/// A workbook with one sheet holding `cells` (reference, value).
fn xlsx_with(cells: &[(&str, u32, f64)]) -> Vec<u8> {
    let mut rows = String::new();
    for (reference, row, value) in cells {
        rows.push_str(&format!(
            r#"<row r="{row}"><c r="{reference}"><v>{value}</v></c></row>"#
        ));
    }
    let parts = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_string(),
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
        ),
        (
            "xl/workbook.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string(),
        ),
        (
            "xl/_rels/workbook.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string(),
        ),
        (
            "xl/worksheets/sheet1.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{rows}</sheetData></worksheet>"#
            ),
        ),
    ];
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, content) in parts {
        writer.start_file(name, options).unwrap();
        writer.write_all(content.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// A BIFF8 record.
fn record(kind: u16, data: &[u8]) -> Vec<u8> {
    let mut out = kind.to_le_bytes().to_vec();
    out.extend_from_slice(&(data.len() as u16).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// An Excel 97-2003 workbook with one sheet holding `cells` (row, column,
/// value), its DIMENSIONS record claiming `dimensions` (rows, columns).
fn xls_with(cells: &[(u16, u16, f64)], dimensions: (u32, u16)) -> Vec<u8> {
    const BOF: u16 = 0x0809;
    const EOF: u16 = 0x000A;
    let bof = |kind: u16| {
        let mut data = vec![0x00, 0x06];
        data.extend_from_slice(&kind.to_le_bytes());
        data.extend_from_slice(&[0; 12]);
        record(BOF, &data)
    };
    let sheet_entry = |position: u32| {
        let mut data = position.to_le_bytes().to_vec();
        data.extend_from_slice(&[0, 0, 1, 0]);
        data.push(b'S');
        record(0x0085, &data)
    };
    let mut globals = bof(0x0005);
    globals.extend(record(0x0042, &1200u16.to_le_bytes()));
    let entry_at = globals.len();
    globals.extend(sheet_entry(0));
    globals.extend(record(EOF, &[]));
    let position = globals.len() as u32;
    globals.splice(
        entry_at..entry_at + sheet_entry(0).len(),
        sheet_entry(position),
    );

    let mut sheet = bof(0x0010);
    let mut dims = 0u32.to_le_bytes().to_vec();
    dims.extend_from_slice(&dimensions.0.to_le_bytes());
    dims.extend_from_slice(&0u16.to_le_bytes());
    dims.extend_from_slice(&dimensions.1.to_le_bytes());
    dims.extend_from_slice(&[0, 0]);
    sheet.extend(record(0x0200, &dims));
    for (row, col, value) in cells {
        let mut data = row.to_le_bytes().to_vec();
        data.extend_from_slice(&col.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&value.to_le_bytes());
        sheet.extend(record(0x0203, &data));
    }
    sheet.extend(record(EOF, &[]));

    let mut stream = globals;
    stream.extend(sheet);
    compound_file(&[("/Workbook", &stream)]).unwrap()
}

#[test]
fn cells_far_apart_are_listed_without_filling_the_grid() {
    // Calamine's ranges are dense: these two cells once took 17 billion
    // positions. They are read one by one and listed by address.
    let data = xlsx_with(&[("A1", 1, 1.0), ("XFD1048576", 1_048_576, 2.0)]);
    let (text, warnings, elapsed) = render_timed(&data, "far.xlsx");
    assert!(text.contains("- A1: 1\n"), "{text}");
    assert!(text.contains("- XFD1048576: 2\n"), "{text}");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
}

#[test]
fn legacy_sheets_spread_over_huge_grids_are_cut_off() {
    // Calamine reads .xls sheets into dense ranges, so cells in opposite
    // corners of a 65536 x 256 sheet would take gigabytes. The reader cuts
    // the sheet off by rows instead, and says so.
    let data = xls_with(&[(0, 0, 1.0), (65_535, 255, 2.0)], (65_536, 256));
    let (text, warnings, elapsed) = render_timed(&data, "far.xls");
    assert!(text.contains("1"), "{text}");
    assert!(!text.contains("IV65536"), "{text}");
    assert!(
        warnings.iter().any(|w| w.contains("was cut off after row")),
        "{warnings:?}"
    );
    assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
}

#[test]
fn lying_dimensions_do_not_reserve_memory() {
    // A DIMENSIONS record claiming the whole sheet while holding one cell.
    let data = xls_with(&[(2, 1, 7.0)], (u32::MAX, u16::MAX));
    let (text, _, elapsed) = render_timed(&data, "lying.xls");
    assert!(
        text.contains("| B |") && text.contains("| 3 | 7 |"),
        "{text}"
    );
    assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
}
