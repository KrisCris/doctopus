//! The PDF reader on small documents built by the tests.

#![allow(missing_docs)]

mod common;

use std::io::Write;
use std::path::PathBuf;

use common::{Doc, PdfBuilder, one_page};
use doctopus_core::render::{self, RenderOptions};
use doctopus_core::{Address, Document, Error, PartKind, ReadOptions};

fn read(data: &[u8]) -> Document {
    common::engine()
        .parse(data, Some("test.pdf"))
        .expect("the document is read")
}

fn markdown(doc: &Document) -> String {
    let address = Address {
        file: PathBuf::from("test.pdf"),
        parts: Vec::new(),
    };
    render::markdown(doc, &RenderOptions::new(address)).text
}

/// The Markdown of a document from its first page heading on.
fn body(doc: &Document) -> String {
    let text = markdown(doc);
    let start = text.find("## Page 1").expect("a first page");
    text[start..].to_string()
}

/// A Word document with one paragraph.
fn docx(text: &str) -> Vec<u8> {
    let files = [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_string(),
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_string(),
        ),
        (
            "word/document.xml",
            format!(
                r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:body></w:document>"#
            ),
        ),
    ];
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in files {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn text_becomes_headings_and_paragraphs() {
    let doc = read(&one_page(
        "BT /F2 24 Tf 72 720 Td (Annual Report) Tj ET \
         BT /F1 12 Tf 72 690 Td (The first paragraph runs over) Tj 0 -14 Td (two lines of text.) Tj ET \
         BT /F1 12 Tf 72 650 Td (Some ) Tj /F2 12 Tf (bold words) Tj /F1 12 Tf (.) Tj ET",
    ));
    assert_eq!(
        body(&doc),
        "## Page 1\n\n### Annual Report\n\n\
         The first paragraph runs over two lines of text.\n\n\
         Some **bold words**.\n"
    );
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
}

#[test]
fn turned_text_reads_along_its_baseline() {
    let doc = read(&one_page(
        "BT /F1 12 Tf 0 1 -1 0 300 200 Tm (Going up the page.) Tj ET \
         BT /F1 12 Tf -1 0 0 -1 400 500 Tm (Upside down.) Tj ET",
    ));
    let text = body(&doc);
    assert!(text.contains("Going up the page."), "{text}");
    assert!(text.contains("Upside down."), "{text}");
}

#[test]
fn links_bookmarks_and_page_labels_are_read() {
    let mut doc = Doc::new(&[
        "BT /F1 12 Tf 72 700 Td (See the site or the next page.) Tj ET",
        "BT /F1 12 Tf 72 700 Td (Second page.) Tj ET",
    ]);
    let (first, next) = (doc.pages[0], doc.pages[1]);
    // Helvetica at 12 points: "site" spans x 116.7 to 135.4, "the next
    // page." 152.7 to 228.8.
    doc.extra[0] = format!(
        "/Annots [<< /Type /Annot /Subtype /Link /Rect [116 695 136 712] \
         /A << /S /URI /URI (https://example.com/) >> >> \
         << /Type /Annot /Subtype /Link /Rect [152 695 230 712] /Dest [{next} 0 R /Fit] >>]"
    );
    let outline = doc.pdf.reserve();
    let [intro, nested, last] = [doc.pdf.reserve(), doc.pdf.reserve(), doc.pdf.reserve()];
    doc.pdf.set(
        outline,
        format!("<< /Type /Outlines /First {intro} 0 R /Last {last} 0 R /Count 2 >>"),
    );
    doc.pdf.set(
        intro,
        format!(
            "<< /Title (Introduction) /Parent {outline} 0 R /Next {last} 0 R \
             /Dest [{first} 0 R /Fit] /First {nested} 0 R /Last {nested} 0 R >>"
        ),
    );
    doc.pdf.set(
        nested,
        format!(
            "<< /Title <FEFF00C9007400E9> /Parent {intro} 0 R \
             /A << /S /GoTo /D [{next} 0 R /XYZ 0 0 0] >> >>"
        ),
    );
    // The last bookmark points back to the first: read once.
    doc.pdf.set(
        last,
        format!(
            "<< /Title (Named) /Parent {outline} 0 R /Prev {intro} 0 R /Next {intro} 0 R \
             /Dest (chapter-2) >>"
        ),
    );
    let doc = read(&doc.finish(
        &format!(
            "/Outlines {outline} 0 R /Names << /Dests << /Names [(chapter-2) [{next} 0 R /Fit]] >> >> \
             /PageLabels << /Nums [0 << /S /r >> 1 << /S /D /P (A-) >>] >>"
        ),
        "",
    ));
    assert_eq!(
        body(&doc),
        "## Page 1\n\n*label: i*\n\n\
         See the [site](https://example.com/) or [the next page.](#page-2)\n\n\
         ## Page 2\n\n*label: A-1*\n\nSecond page.\n\n\
         ## Bookmarks\n\n\
         - [Introduction](#page-1)\n  - [Été](#page-2)\n- [Named](#page-2)\n"
    );
}

#[test]
fn metadata_comes_from_the_information_dictionary_and_xmp() {
    let mut doc = Doc::new(&["BT /F1 12 Tf 72 700 Td (Text.) Tj ET"]);
    let xmp = doc.pdf.stream(
        "/Type /Metadata /Subtype /XML",
        br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:CreatorTool="Writer &amp; Co">
<dc:title><rdf:Alt><rdf:li xml:lang="de">Titel</rdf:li><rdf:li xml:lang="x-default">Ignored title</rdf:li></rdf:Alt></dc:title>
<dc:creator><rdf:Seq><rdf:li>Ada</rdf:li><rdf:li>Charles</rdf:li></rdf:Seq></dc:creator>
</rdf:Description></rdf:RDF></x:xmpmeta>"#,
    );
    let doc = read(&doc.finish(
        &format!("/Metadata {xmp} 0 R /Lang (en-GB)"),
        "/Info << /Title (Quarterly \\(draft\\)) /Producer <FEFF00C9> \
         /CreationDate (D:20240102030405+01'30') /ModDate (D:20240203) >>",
    ));
    let fields: Vec<(&str, &str)> = doc.metadata.fields().collect();
    assert_eq!(
        fields,
        [
            ("title", "Quarterly (draft)"),
            ("author", "Ada; Charles"),
            ("created", "2024-01-02T03:04:05+01:30"),
            ("modified", "2024-02-03T00:00:00"),
            ("application", "Writer & Co"),
            ("language", "en-GB"),
            // Names that the model does not know come last, sorted.
            ("pdf_version", "1.7"),
            ("producer", "É"),
        ]
    );
}

#[test]
fn form_fields_are_listed_with_their_values() {
    let mut doc = Doc::new(&["BT /F1 12 Tf 72 700 Td (A form.) Tj ET"]);
    let name = doc.pdf.add("<< /FT /Tx /T (name) /V (Ada Lovelace) >>");
    let agree = doc.pdf.add("<< /FT /Btn /T (agree) /V /Yes >>");
    let address = doc.pdf.reserve();
    let city = doc.pdf.add(format!(
        "<< /T (city) /Parent {address} 0 R /V <FEFF004F0073006C006F> >>"
    ));
    doc.pdf.set(
        address,
        format!("<< /FT /Tx /T (address) /Kids [{city} 0 R] >>"),
    );
    let off = doc.pdf.add("<< /FT /Btn /T (newsletter) /V /Off >>");
    let button = doc.pdf.add("<< /FT /Btn /Ff 65536 /T (submit) >>");
    let doc = read(&doc.finish(
        &format!(
            "/AcroForm << /Fields [{name} 0 R {agree} 0 R {address} 0 R {off} 0 R {button} 0 R] >>"
        ),
        "",
    ));
    let text = body(&doc);
    let form = &text[text.find("## Form fields").expect("form fields")..];
    assert_eq!(
        form,
        "## Form fields\n\n| Field | Type | Value |\n|---|---|---|\n\
         | name | text | Ada Lovelace |\n| agree | check box | Yes |\n\
         | address.city | text | Oslo |\n| newsletter | check box |  |\n"
    );
}

#[test]
fn attached_files_are_read_through() {
    let mut doc = Doc::new(&["BT /F1 12 Tf 72 700 Td (See the attached files.) Tj ET"]);
    let inner = one_page("BT /F1 12 Tf 72 700 Td (Inside the attachment.) Tj ET");
    let pdf = doc
        .pdf
        .deflated("/Type /EmbeddedFile /Subtype /application#2Fpdf", &inner);
    let word = doc
        .pdf
        .stream("/Type /EmbeddedFile", &docx("Hello from Word."));
    // The Unicode name wins over the path in /F.
    doc.extra[0] = format!(
        "/Annots [<< /Type /Annot /Subtype /FileAttachment /Rect [300 690 320 710] \
         /FS << /Type /Filespec /F (C:\\\\docs\\\\old.docx) \
         /UF <FEFF006C00650074007400650072002E0064006F00630078> /EF << /F {word} 0 R >> >> >>]"
    );
    let doc = read(&doc.finish(
        &format!(
            "/Names << /EmbeddedFiles << /Names [(inner) << /Type /Filespec /F (dir/inner.pdf) \
             /Desc (The inner one) /EF << /F {pdf} 0 R >> >>] >> >>"
        ),
        "",
    ));
    let [letter, attached] = &doc.parts[..] else {
        panic!("two parts: {:?}", doc.parts);
    };
    assert_eq!(letter.name.as_deref(), Some("letter.docx"));
    assert_eq!(letter.location.as_deref(), Some("page 1"));
    assert_eq!(attached.name.as_deref(), Some("inner.pdf"));
    assert_eq!(attached.description.as_deref(), Some("The inner one"));
    assert_eq!(attached.content_type.as_deref(), Some("application/pdf"));
    let text = body(&doc);
    // The attachment annotation's file is shown on its page, the others
    // after the pages.
    let page = text.find("See the attached files.").unwrap();
    let word = text.find("Hello from Word.").expect("the Word text");
    let other = text.find("## Other embedded content").unwrap();
    let pdf = text.find("Inside the attachment.").expect("the PDF text");
    assert!(page < word && word < other && other < pdf, "{text}");
}

#[test]
fn pictures_become_parts_and_scans_are_reported() {
    let mut doc = Doc::new(&[
        "q 612 0 0 792 0 0 cm /Scan Do Q",
        "BT /F1 12 Tf 72 700 Td (A photo:) Tj ET q 100 0 0 50 72 600 cm /Photo Do Q \
         q 4 0 0 4 72 590 cm /Photo Do Q \
         BT /F1 12 Tf 72 560 Td (Below the photo.) Tj ET",
        "q 200 0 0 100 72 600 cm /Logo Do Q",
    ]);
    let pixels: Vec<u8> = (0..64 * 64).map(|i| (i % 251) as u8).collect();
    let scan = doc.pdf.deflated(
        "/Type /XObject /Subtype /Image /Width 64 /Height 64 /ColorSpace /DeviceGray \
         /BitsPerComponent 8",
        &pixels,
    );
    // The header of a JPEG is enough: JPEG data is passed through.
    let jpeg = [
        0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x10, 0x00, 0x20, 0x01, 0x01, 0x11, 0x00,
        0xFF, 0xD9,
    ];
    let photo = doc.pdf.stream(
        "/Type /XObject /Subtype /Image /Width 32 /Height 16 /ColorSpace /DeviceGray \
         /BitsPerComponent 8 /Filter /DCTDecode",
        &jpeg,
    );
    // Some writers deflate JPEG data once more.
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(&jpeg).unwrap();
    let logo = doc.pdf.stream(
        "/Type /XObject /Subtype /Image /Width 32 /Height 16 /ColorSpace /DeviceGray \
         /BitsPerComponent 8 /Filter [/FlateDecode /DCTDecode]",
        &encoder.finish().unwrap(),
    );
    doc.resources[0] = format!("/XObject << /Scan {scan} 0 R >>");
    doc.resources[1] = format!("/XObject << /Photo {photo} 0 R >>");
    doc.resources[2] = format!("/XObject << /Logo {logo} 0 R >>");
    let doc = read(&doc.finish("", ""));
    let [scan, photo, logo] = &doc.parts[..] else {
        panic!("three pictures: {:?}", doc.parts);
    };
    assert_eq!(scan.kind, PartKind::Image);
    assert_eq!(scan.content_type.as_deref(), Some("image/png"));
    assert_eq!(scan.dimensions, Some((64, 64)));
    assert_eq!(photo.content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(photo.data, jpeg);
    assert_eq!(logo.content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(logo.data, jpeg);
    assert_eq!(
        doc.warnings,
        [
            "2 pages have no text layer, only pictures (scans, or text set as pictures); \
          extract the pictures to see them: page 1 (img1), page 3 (img3)"
        ]
    );
    // The photo is placed between the paragraphs, once: drawn again as a
    // dot, it is left out.
    assert_eq!(
        body(&doc),
        "## Page 1\n\n![image](test.pdf#img1 \"PNG, 64x64, 1.7 KB\")\n\n\
         ## Page 2\n\nA photo:\n\n![image](test.pdf#img2 \"JPG, 32x16, 17 B\")\n\n\
         Below the photo.\n\n\
         ## Page 3\n\n![image](test.pdf#img3 \"JPG, 32x16, 17 B\")\n"
    );
}

#[test]
fn a_picture_that_decodes_to_too_much_is_left_out() {
    let mut doc = Doc::new(&[
        "BT /F1 12 Tf 72 700 Td (Text survives.) Tj ET q 100 0 0 100 72 400 cm /Im Do Q",
    ]);
    let image = doc.pdf.deflated(
        "/Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceGray \
         /BitsPerComponent 8",
        &vec![0; 4 << 20],
    );
    doc.resources[0] = format!("/XObject << /Im {image} 0 R >>");
    let doc = read(&doc.finish("", ""));
    assert!(doc.parts.is_empty());
    assert_eq!(body(&doc), "## Page 1\n\nText survives.\n");
    assert_eq!(
        doc.warnings,
        [
            "page 1: a picture was left out because its data decodes to far more than its dimensions need"
        ]
    );
}

#[test]
fn a_stream_that_expands_too_far_is_refused() {
    let mut doc = Doc::new(&["BT /F1 12 Tf 72 700 Td (Fine.) Tj ET"]);
    let bomb = doc.pdf.deflated("", &vec![b' '; 8 << 20]);
    doc.extra[0] = format!("/Thumb {bomb} 0 R");
    let mut options = ReadOptions::default();
    options.max_part_size = 1 << 20;
    let err = common::engine_with(options)
        .parse(&doc.finish("", ""), Some("test.pdf"))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("a stream expands to more than 1 MiB"),
        "{err}"
    );
}

#[test]
fn encrypted_documents_are_reported() {
    let mut pdf = PdfBuilder::new();
    let root = pdf.add("<< /Type /Catalog /Pages 2 0 R >>");
    pdf.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    pdf.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>");
    // A user password other than the empty one.
    let encrypt = pdf.add(
        "<< /Filter /Standard /V 2 /R 3 /Length 128 /P -4 \
         /O <0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF> \
         /U <FEDCBA9876543210FEDCBA9876543210FEDCBA9876543210FEDCBA9876543210> >>",
    );
    let id = "<00112233445566778899AABBCCDDEEFF>";
    let data = pdf.finish(root, &format!("/Encrypt {encrypt} 0 R /ID [{id} {id}]"));
    let err = common::engine().parse(&data, Some("test.pdf")).unwrap_err();
    assert!(matches!(err, Error::Encrypted { .. }), "{err:?}");
}

#[test]
fn a_damaged_cross_reference_table_is_rebuilt() {
    let mut data = one_page("BT /F1 12 Tf 72 700 Td (Still readable.) Tj ET");
    let at = data.windows(9).rposition(|w| w == b"startxref").unwrap();
    data.truncate(at);
    data.extend_from_slice(b"startxref\n12345\n%%EOF\n");
    assert_eq!(body(&read(&data)), "## Page 1\n\nStill readable.\n");
}

#[test]
fn a_file_with_no_pages_is_malformed() {
    let err = common::engine()
        .parse(b"%PDF-1.4\nnothing else", Some("test.pdf"))
        .unwrap_err();
    assert!(matches!(err, Error::Malformed { .. }), "{err:?}");
}
