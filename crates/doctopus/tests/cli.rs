//! The `doctopus` binary end to end: arguments, paging, files and exit
//! codes.

#![allow(missing_docs)]

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A directory for one test, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("doctopus-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn run(&self, args: &[&str]) -> Run {
        let output = Command::new(env!("CARGO_BIN_EXE_doctopus"))
            .args(args)
            .current_dir(&self.0)
            .env_remove("DOCTOPUS_MAX_CHARS")
            .output()
            .unwrap();
        Run(output)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Run(Output);

impl Run {
    fn code(&self) -> i32 {
        self.0.status.code().unwrap()
    }

    fn stdout(&self) -> String {
        String::from_utf8(self.0.stdout.clone()).unwrap()
    }

    fn stderr(&self) -> String {
        String::from_utf8(self.0.stderr.clone()).unwrap()
    }
}

/// A workbook with sheets of `rows` numbered text rows each.
fn workbook(sheets: &[(&str, usize)]) -> Vec<u8> {
    let mut parts = vec![
        (
            "_rels/.rels".to_string(),
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string(),
        ),
    ];
    let mut types = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>"#,
    );
    let mut entries = String::new();
    let mut rels = String::new();
    for (i, (name, rows)) in sheets.iter().enumerate() {
        let n = i + 1;
        types.push_str(&format!(r#"<Override PartName="/xl/worksheets/sheet{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#));
        entries.push_str(&format!(
            r#"<sheet name="{name}" sheetId="{n}" r:id="rId{n}"/>"#
        ));
        rels.push_str(&format!(r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{n}.xml"/>"#));
        let data: String = (1..=*rows)
            .map(|r| format!(r#"<row r="{r}"><c r="A{r}"><v>{r}</v></c><c r="B{r}" t="inlineStr"><is><t>{name} row {r}</t></is></c></row>"#))
            .collect();
        parts.push((
            format!("xl/worksheets/sheet{n}.xml"),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>{data}</sheetData></worksheet>"#),
        ));
    }
    types.push_str("</Types>");
    parts.push(("[Content_Types].xml".into(), types));
    parts.push((
        "xl/workbook.xml".into(),
        format!(r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{entries}</sheets></workbook>"#),
    ));
    parts.push((
        "xl/_rels/workbook.xml.rels".into(),
        format!(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#),
    ));
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in parts {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(content.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// A Word document with the given body, and paragraph styles for
/// headings, quotations and code.
fn word_document(body: &str) -> Vec<u8> {
    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
    let style = |id: &str, name: &str, ppr: &str| {
        format!(
            r#"<w:style w:type="paragraph" w:styleId="{id}"><w:name w:val="{name}"/>{ppr}</w:style>"#
        )
    };
    let styles = format!(
        r#"<w:styles {W}>{}{}{}{}</w:styles>"#,
        style(
            "Heading1",
            "heading 1",
            r#"<w:pPr><w:outlineLvl w:val="0"/></w:pPr>"#
        ),
        style(
            "Heading2",
            "heading 2",
            r#"<w:pPr><w:outlineLvl w:val="1"/></w:pPr>"#
        ),
        style("Quote", "Quote", ""),
        style("Code", "Code", ""),
    );
    let document = format!(r#"<w:document {W}><w:body>{body}</w:body></w:document>"#);
    let package = "http://schemas.openxmlformats.org/package/2006";
    let office = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let main = "application/vnd.openxmlformats-officedocument.wordprocessingml";
    let files = [
        (
            "[Content_Types].xml",
            format!(
                r#"<Types xmlns="{package}/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="{main}.document.main+xml"/></Types>"#
            ),
        ),
        (
            "_rels/.rels",
            format!(
                r#"<Relationships xmlns="{package}/relationships"><Relationship Id="rId1" Type="{office}/officeDocument" Target="word/document.xml"/></Relationships>"#
            ),
        ),
        (
            "word/_rels/document.xml.rels",
            format!(
                r#"<Relationships xmlns="{package}/relationships"><Relationship Id="rId1" Type="{office}/styles" Target="styles.xml"/></Relationships>"#
            ),
        ),
        ("word/document.xml", document),
        ("word/styles.xml", styles),
    ];
    let files: Vec<(&str, &[u8])> = files.iter().map(|(n, c)| (*n, c.as_bytes())).collect();
    archive(&files)
}

/// A zip archive holding `files`, each a (name, bytes) pair.
fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// The local link targets in a Markdown text, leaving out code blocks and
/// escaped `\](`.
fn local_links(markdown: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut code = false;
    for line in markdown.lines() {
        if line.starts_with("```") {
            code = !code;
        }
        if code {
            continue;
        }
        for (i, _) in line.match_indices("](") {
            if line[..i].ends_with('\\') {
                continue;
            }
            let rest = &line[i + 2..];
            let target = match rest.strip_prefix('<') {
                Some(rest) => rest.split('>').next(),
                None => rest.split([' ', ')']).next(),
            };
            if let Some(target) = target.filter(|t| !t.contains("://")) {
                links.push(target.to_string());
            }
        }
    }
    links
}

/// The offset in a page note's "next page" command.
fn next_offset(note: &str) -> Option<usize> {
    let line = note.lines().find(|l| l.starts_with("-- next page:"))?;
    let words: Vec<&str> = line.split_whitespace().collect();
    let at = words.iter().position(|w| *w == "--offset")?;
    words.get(at + 1)?.parse().ok()
}

#[test]
fn help_version_and_usage_errors() {
    let dir = Scratch::new("help");
    let none = dir.run(&[]);
    assert_eq!(none.code(), 2);
    assert!(
        none.stderr().contains("Usage: doctopus"),
        "{}",
        none.stderr()
    );
    let version = dir.run(&["--version"]);
    assert_eq!(version.code(), 0);
    assert!(
        version.stdout().contains(env!("CARGO_PKG_VERSION")),
        "{}",
        version.stdout()
    );
    assert_eq!(dir.run(&["read", "--bogus", "x.docx"]).code(), 2);
    let guide = dir.run(&["guide"]);
    assert_eq!(guide.code(), 0);
    assert!(guide.stdout().starts_with("# doctopus guide"));
}

#[test]
fn exit_codes_tell_failures_apart() {
    let dir = Scratch::new("exit-codes");
    let missing = dir.run(&["read", "missing.docx"]);
    assert_eq!(missing.code(), 1);
    assert!(
        missing
            .stderr()
            .starts_with("error: cannot read `missing.docx`")
    );
    assert!(missing.stderr().contains("\nhint: "));

    fs::write(dir.path("data.bin"), [0, 1, 2, 3]).unwrap();
    let unsupported = dir.run(&["read", "data.bin"]);
    assert_eq!(unsupported.code(), 3);
    assert!(unsupported.stderr().contains("doctopus formats"));

    fs::write(dir.path("book.xlsx"), workbook(&[("Data", 2)])).unwrap();
    let no_sheet = dir.run(&["read", "book.xlsx", "--sheet", "9"]);
    assert_eq!(no_sheet.code(), 2);
    assert!(
        no_sheet.stderr().contains(r#"available: sheet 1 "Data""#),
        "{}",
        no_sheet.stderr()
    );
    let no_part = dir.run(&["read", "book.xlsx#emb1"]);
    assert_eq!(no_part.code(), 2);
    assert!(
        no_part.stderr().contains("no part `emb1`"),
        "{}",
        no_part.stderr()
    );
}

#[test]
fn a_file_argument_alone_is_read() {
    let dir = Scratch::new("implicit-read");
    fs::write(dir.path("book.xlsx"), workbook(&[("Data", 3)])).unwrap();
    let implicit = dir.run(&["book.xlsx"]);
    let explicit = dir.run(&["read", "book.xlsx"]);
    assert_eq!(implicit.code(), 0, "{}", implicit.stderr());
    assert_eq!(implicit.stdout(), explicit.stdout());
    assert!(implicit.stdout().contains("| 3 | 3 | Data row 3 |"));
    assert!(implicit.stderr().is_empty(), "{}", implicit.stderr());
}

#[test]
fn pages_join_up_to_the_whole_output() {
    let dir = Scratch::new("paging");
    fs::write(
        dir.path("book.xlsx"),
        workbook(&[("First", 40), ("Second", 40)]),
    )
    .unwrap();
    let whole = dir.run(&["read", "book.xlsx", "--max-chars", "0"]).stdout();
    let mut joined = String::new();
    let mut offset = 0;
    for _ in 0..100 {
        let page = dir.run(&[
            "read",
            "book.xlsx",
            "--max-chars",
            "500",
            "--offset",
            &offset.to_string(),
        ]);
        assert_eq!(page.code(), 0, "{}", page.stderr());
        joined.push_str(&page.stdout());
        match next_offset(&page.stderr()) {
            Some(next) => {
                assert!(
                    page.stderr().contains("--max-chars 500"),
                    "{}",
                    page.stderr()
                );
                offset = next;
            }
            None => {
                assert!(
                    page.stderr().contains("-- end of output"),
                    "{}",
                    page.stderr()
                );
                break;
            }
        }
    }
    assert_eq!(joined, whole);
}

#[test]
fn output_files_come_with_a_map_of_sections() {
    let dir = Scratch::new("output-file");
    fs::write(
        dir.path("book.xlsx"),
        workbook(&[("First", 5), ("Second", 5)]),
    )
    .unwrap();
    let run = dir.run(&["read", "book.xlsx", "-o", "out/book.md"]);
    assert_eq!(run.code(), 0, "{}", run.stderr());
    let written = fs::read_to_string(dir.path("out/book.md")).unwrap();
    let map = run.stdout();
    let line: usize = map
        .lines()
        .find_map(|l| l.strip_suffix(r#": sheet 2 "Second""#))
        .and_then(|l| l.trim().strip_prefix("- "))
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no line for sheet 2 in:\n{map}"));
    assert_eq!(written.lines().nth(line - 1), Some("## Sheet 2: Second"));
}

#[test]
fn info_surveys_several_files() {
    let dir = Scratch::new("info");
    fs::write(dir.path("a.xlsx"), workbook(&[("Data", 2)])).unwrap();
    let json = dir.run(&["info", "--json", "a.xlsx"]);
    assert_eq!(json.code(), 0);
    assert!(json.stdout().starts_with("{\n  \"source\": \"a.xlsx\""));

    let several = dir.run(&["info", "a.xlsx", "missing.xlsx"]);
    assert_eq!(several.code(), 1);
    let out = several.stdout();
    assert!(out.contains("source: a.xlsx\n"), "{out}");
    assert!(
        out.contains("---\nsource: missing.xlsx\nerror: cannot read"),
        "{out}"
    );
}

#[test]
fn embedded_parts_are_addressed_and_extracted() {
    // Uses a corpus file with nested embedded documents when it has been
    // fetched (scripts/fetch-corpus.sh).
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/corpus/tika/testEXCEL_embeded.xlsx");
    if !corpus.is_file() {
        return;
    }
    let dir = Scratch::new("parts");
    fs::copy(&corpus, dir.path("book.xlsx")).unwrap();

    let embedded = dir.run(&["read", "book.xlsx#emb1", "--no-front-matter"]);
    assert_eq!(embedded.code(), 0, "{}", embedded.stderr());
    assert!(
        embedded.stdout().starts_with("## Slide 1"),
        "{}",
        embedded.stdout()
    );

    let image = dir.run(&["read", "book.xlsx#img1"]);
    assert_eq!(image.code(), 2);
    assert!(
        image.stderr().contains("doctopus extract"),
        "{}",
        image.stderr()
    );

    let extract = dir.run(&["extract", "book.xlsx", "--json"]);
    assert_eq!(extract.code(), 0, "{}", extract.stderr());
    assert!(dir.path("book.parts/image1.png").is_file());
    assert!(dir.path("book.parts/emb1/emb2/image3.jpeg").is_file());
    assert!(
        extract
            .stdout()
            .contains("\"address\": \"book.xlsx#emb1/emb2/img2\"")
    );

    let with_files = dir.run(&["read", "book.xlsx", "--extract", "saved"]);
    assert!(
        with_files.stdout().contains("](saved/image1.png"),
        "{}",
        with_files.stdout()
    );

    // Written to a file, links are relative to that file.
    let to_file = dir.run(&[
        "read",
        "book.xlsx",
        "-o",
        "out/book.md",
        "--extract",
        "out/files",
    ]);
    assert_eq!(to_file.code(), 0, "{}", to_file.stderr());
    let written = fs::read_to_string(dir.path("out/book.md")).unwrap();
    assert!(written.contains("](files/image1.png"), "{written}");
}

#[test]
fn convert_writes_markdown_for_every_document_inside() {
    let dir = Scratch::new("convert");
    let inner = archive(&[("deep/notes.txt", b"see [this](that)\n")]);
    let bundle = archive(&[
        ("sheets/book.xlsx", &workbook(&[("Data", 2)])),
        ("inner.zip", &inner),
        ("logo.bin", &[0, 1, 2, 3]),
    ]);
    fs::write(dir.path("bundle.zip"), &bundle).unwrap();

    let run = dir.run(&["convert", "bundle.zip", "-o", "out"]);
    assert_eq!(run.code(), 0, "{}", run.stderr());
    let report = run.stdout();
    assert!(report.contains("markdown: # 4\n"), "{report}");
    // The path is native, and quoted where it has a backslash.
    let next = report.lines().find(|l| l.starts_with("next: read "));
    assert!(next.is_some_and(|l| l.contains("bundle.md")), "{report}");

    let top = fs::read_to_string(dir.path("out/bundle.md")).unwrap();
    assert!(top.contains("](bundle.parts/emb1/book.md "), "{top}");
    assert!(top.contains("](bundle.parts/emb2/inner.md "), "{top}");
    assert!(top.contains("](bundle.parts/logo.bin "), "{top}");
    assert!(!top.contains("doctopus "), "{top}");
    let book = fs::read_to_string(dir.path("out/bundle.parts/emb1/book.md")).unwrap();
    assert!(book.contains("Data row 2"), "{book}");
    let notes = dir.path("out/bundle.parts/emb2/emb1/notes.md");
    let notes = fs::read_to_string(notes).unwrap();
    assert!(notes.contains("```\nsee [this](that)\n```"), "{notes}");

    // Every link from every Markdown file leads to a file that was written.
    let json = dir.run(&["convert", "bundle.zip", "-o", "again", "--json"]);
    assert_eq!(json.code(), 0, "{}", json.stderr());
    let value: serde_json::Value = serde_json::from_str(&json.stdout()).unwrap();
    let files = value["markdown"].as_array().unwrap();
    assert_eq!(files.len(), 4);
    for file in files {
        let file = dir.path(file["file"].as_str().unwrap());
        let markdown = fs::read_to_string(&file).unwrap();
        for link in local_links(&markdown) {
            let target = file.parent().unwrap().join(&link);
            assert!(target.is_file(), "{}: broken link {link}", file.display());
        }
    }

    // Two inputs with the same name do not overwrite each other.
    fs::create_dir_all(dir.path("copy")).unwrap();
    fs::write(dir.path("copy/bundle.zip"), &bundle).unwrap();
    let both = dir.run(&["convert", "bundle.zip", "copy/bundle.zip", "-o", "both"]);
    assert_eq!(both.code(), 0, "{}", both.stderr());
    assert!(dir.path("both/bundle.md").is_file());
    assert!(dir.path("both/bundle-2.md").is_file());
}

#[test]
fn converted_word_documents_keep_their_formatting() {
    let p = |style: &str, runs: &str| {
        format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr>{runs}</w:p>"#)
    };
    let r = |text: &str| format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#);
    let body = [
        p("Normal", &format!(
            r#"<w:hyperlink w:anchor="_Toc1">{}</w:hyperlink>"#,
            r("Getting started")
        )),
        p("Heading1", &format!(
            r#"<w:bookmarkStart w:id="1" w:name="_Toc1"/>{}<w:bookmarkEnd w:id="1"/>"#,
            r("Getting started")
        )),
        p("Normal", &format!(
            r#"{}<w:r><w:rPr><w:rFonts w:ascii="Consolas"/></w:rPr><w:t>make</w:t></w:r>{}<w:r><w:br/></w:r>{}"#,
            r("Run "),
            r(" first."),
            r("Then wait.")
        )),
        p("Heading2", &r("Details")),
        p("Quote", &r("Well begun is half done.")),
        p("Code", &r("make all")),
        p("Code", &r("    make install")),
    ]
    .concat();
    let dir = Scratch::new("formatting");
    fs::write(dir.path("guide.docx"), word_document(&body)).unwrap();
    let run = dir.run(&["convert", "guide.docx", "-o", "out"]);
    assert_eq!(run.code(), 0, "{}", run.stderr());
    let markdown = fs::read_to_string(dir.path("out/guide.md")).unwrap();
    let body = markdown.split("---\n\n").nth(1).unwrap_or_default();
    assert_eq!(
        body,
        "[Getting started](#getting-started)\n\n\
         # Getting started\n\n\
         Run `make` first.\\\nThen wait.\n\n\
         ## Details\n\n\
         > Well begun is half done.\n\n\
         ```\nmake all\n    make install\n```\n"
    );
}
