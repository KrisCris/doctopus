//! Helpers shared by the integration tests.

#![allow(dead_code, unreachable_pub, clippy::print_stderr)]

use std::path::{Path, PathBuf};

use doctopus_core::{Document, Engine, Probe, ReadOptions, Registry};

/// A registry with the PDF reader only.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    doctopus_pdf::register(&mut registry);
    registry
}

/// An engine with the PDF reader and the Office readers, so that files
/// attached to PDFs and PDFs embedded in Office files are read through.
pub fn engine() -> Engine {
    engine_with(ReadOptions::default())
}

pub fn engine_with(options: ReadOptions) -> Engine {
    let mut registry = Registry::new();
    doctopus_pdf::register(&mut registry);
    doctopus_office::register(&mut registry);
    Engine::new(registry).with_options(options)
}

/// One file of the third-party test corpus.
#[derive(Debug)]
pub struct CorpusFile {
    /// `source/file`, as listed in `testdata/corpus.tsv`.
    pub name: String,
    pub path: PathBuf,
}

/// The files listed in `testdata/corpus.tsv`, or `None` when the corpus
/// has not been fetched (`scripts/fetch-corpus.sh`). A partly fetched corpus
/// fails the test instead of silently testing less.
pub fn corpus_files() -> Option<Vec<CorpusFile>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let manifest = std::fs::read_to_string(root.join("corpus.tsv")).expect("testdata/corpus.tsv");
    let files: Vec<CorpusFile> = manifest
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let mut fields = line.split('\t');
            let source = fields.next().expect("source column");
            let file = fields.next().expect("file column");
            CorpusFile {
                name: format!("{source}/{file}"),
                path: root.join("corpus").join(source).join(file),
            }
        })
        .collect();
    if !root.join("corpus").is_dir() {
        eprintln!("skipped: the test corpus is missing; fetch it with scripts/fetch-corpus.sh");
        return None;
    }
    let missing: Vec<&str> = files
        .iter()
        .filter(|f| !f.path.is_file())
        .map(|f| f.name.as_str())
        .collect();
    assert!(
        missing.is_empty(),
        "corpus files missing (run scripts/fetch-corpus.sh): {missing:?}"
    );
    Some(files)
}

/// The PDF files of the corpus.
pub fn corpus() -> Option<Vec<CorpusFile>> {
    let registry = registry();
    let files = corpus_files()?;
    Some(
        files
            .into_iter()
            .filter(|f| {
                let data = std::fs::read(&f.path).expect("corpus file");
                registry.detect(&Probe::new(&data, Some(&f.name))).is_some()
            })
            .collect(),
    )
}

/// PDF documents embedded in the other files of the corpus, at any depth,
/// as `(name, data)`.
pub fn embedded_pdfs() -> Vec<(String, Vec<u8>)> {
    fn collect(name: &str, doc: &Document, registry: &Registry, out: &mut Vec<(String, Vec<u8>)>) {
        for part in &doc.parts {
            let name = format!("{name}#{}", part.id);
            if registry.detect(&Probe::new(&part.data, None)).is_some() {
                out.push((name.clone(), part.data.clone()));
            }
            if let Some(inner) = &part.document {
                collect(&name, inner, registry, out);
            }
        }
    }
    let Some(files) = corpus_files() else {
        return Vec::new();
    };
    let (engine, registry) = (engine(), registry());
    let mut out = Vec::new();
    for file in files {
        let data = std::fs::read(&file.path).expect("corpus file");
        if registry
            .detect(&Probe::new(&data, Some(&file.name)))
            .is_some()
        {
            continue;
        }
        if let Ok(doc) = engine.parse(&data, Some(&file.name)) {
            collect(&file.name, &doc, &registry, &mut out);
        }
    }
    out
}

/// A PDF assembled object by object.
#[derive(Debug, Default)]
pub struct PdfBuilder {
    objects: Vec<Vec<u8>>,
}

impl PdfBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an object, given its body, and return its number.
    pub fn add(&mut self, body: impl AsRef<[u8]>) -> usize {
        self.objects.push(body.as_ref().to_vec());
        self.objects.len()
    }

    /// Reserve an object number to fill in later with [`set`](Self::set).
    pub fn reserve(&mut self) -> usize {
        self.add("null")
    }

    pub fn set(&mut self, number: usize, body: impl AsRef<[u8]>) {
        self.objects[number - 1] = body.as_ref().to_vec();
    }

    /// Add a stream with the given dictionary entries (without `/Length`).
    pub fn stream(&mut self, entries: &str, data: &[u8]) -> usize {
        let mut body = format!("<< {entries} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.add(body)
    }

    /// Add a stream compressed with deflate.
    pub fn deflated(&mut self, entries: &str, data: &[u8]) -> usize {
        use std::io::Write;
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(data).unwrap();
        let compressed = encoder.finish().unwrap();
        self.stream(&format!("{entries} /Filter /FlateDecode"), &compressed)
    }

    /// The file, with `root` as its catalog and `trailer` as extra trailer
    /// entries.
    pub fn finish(&self, root: usize, trailer: &str) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", self.objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root {root} 0 R {trailer} >>\nstartxref\n{xref}\n%%EOF\n",
                self.objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }
}

/// A document under construction whose pages draw given content streams,
/// with Helvetica as font `/F1` and Helvetica-Bold as `/F2`. Page
/// dictionaries are written by [`finish`](Self::finish), so their extra
/// entries may refer to objects added later.
#[derive(Debug)]
pub struct Doc {
    pub pdf: PdfBuilder,
    /// Object numbers of the pages.
    pub pages: Vec<usize>,
    /// Extra entries of each page dictionary.
    pub extra: Vec<String>,
    /// Extra entries of each page's font resources.
    pub fonts: Vec<String>,
    /// Extra entries of each page's resource dictionary.
    pub resources: Vec<String>,
    contents: Vec<usize>,
    catalog: usize,
    tree: usize,
    standard_fonts: String,
}

impl Doc {
    /// Pages drawing the given content streams.
    pub fn new(contents: &[&str]) -> Self {
        let mut pdf = PdfBuilder::new();
        let catalog = pdf.reserve();
        let tree = pdf.reserve();
        let font = pdf.add(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        );
        let bold = pdf.add(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>",
        );
        let contents: Vec<usize> = contents
            .iter()
            .map(|content| pdf.stream("", content.as_bytes()))
            .collect();
        let pages = contents.iter().map(|_| pdf.reserve()).collect();
        Self {
            pdf,
            pages,
            extra: vec![String::new(); contents.len()],
            fonts: vec![String::new(); contents.len()],
            resources: vec![String::new(); contents.len()],
            contents,
            catalog,
            tree,
            standard_fonts: format!("/F1 {font} 0 R /F2 {bold} 0 R"),
        }
    }

    /// The file, with extra catalog and trailer entries.
    pub fn finish(mut self, catalog: &str, trailer: &str) -> Vec<u8> {
        let tree = self.tree;
        for (i, &page) in self.pages.iter().enumerate() {
            self.pdf.set(
                page,
                format!(
                    "<< /Type /Page /Parent {tree} 0 R /MediaBox [0 0 612 792] /Contents {} 0 R \
                     /Resources << /Font << {} {} >> {} >> {} >>",
                    self.contents[i],
                    self.standard_fonts,
                    self.fonts[i],
                    self.resources[i],
                    self.extra[i]
                ),
            );
        }
        let kids: Vec<String> = self.pages.iter().map(|k| format!("{k} 0 R")).collect();
        self.pdf.set(
            tree,
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                kids.join(" "),
                kids.len()
            ),
        );
        self.pdf.set(
            self.catalog,
            format!("<< /Type /Catalog /Pages {tree} 0 R {catalog} >>"),
        );
        self.pdf.finish(self.catalog, trailer)
    }
}

/// A one-page document drawing `content`.
pub fn one_page(content: &str) -> Vec<u8> {
    Doc::new(&[content]).finish("", "")
}
