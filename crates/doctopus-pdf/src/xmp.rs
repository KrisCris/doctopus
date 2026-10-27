//! XMP metadata: the RDF/XML packet many PDF writers store beside, or
//! instead of, the document information dictionary.

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};

/// Properties read from XMP, by doctopus metadata name, in packet order.
/// The first value of a property wins, a default-language one over others;
/// the items of list values (authors, keywords) are joined.
pub(crate) fn properties(data: &[u8]) -> Vec<(&'static str, String)> {
    let packet = String::from_utf8_lossy(data);
    let mut reader = Reader::from_str(&packet);
    let mut out = Properties::default();
    let mut current: Option<(&'static str, bool)> = None;
    let mut lang: Option<String> = None;
    let mut text = String::new();
    loop {
        let event = match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(event) => event,
        };
        match event {
            Event::Start(e) => {
                let local = e.local_name();
                if let Some(p) = property(local.as_ref()) {
                    current = Some(p);
                    text.clear();
                } else if local.as_ref() == "li" {
                    lang = xml_lang(&e);
                    text.clear();
                } else if local.as_ref() == "Description" {
                    out.attributes(&e);
                }
            }
            Event::Empty(e) if e.local_name().as_ref() == "Description" => out.attributes(&e),
            Event::Text(t) if current.is_some() => text.push_str(&t.xml10_content()),
            Event::CData(t) if current.is_some() => text.push_str(&t.into_inner()),
            Event::GeneralRef(r) if current.is_some() => match r.resolve_char_ref() {
                Ok(Some(c)) => text.push(c),
                _ => text.push_str(
                    quick_xml::escape::resolve_predefined_entity(&r.into_inner())
                        .unwrap_or_default(),
                ),
            },
            Event::End(e) => {
                let local = e.local_name();
                if local.as_ref() == "li" {
                    if let Some((name, list)) = current {
                        out.add(name, &text, list, lang.as_deref());
                    }
                    text.clear();
                    lang = None;
                } else if let Some((name, list)) = property(local.as_ref()) {
                    out.add(name, &text, list, None);
                    current = None;
                    text.clear();
                }
            }
            _ => {}
        }
    }
    out.0
        .into_iter()
        .map(|(name, value, _)| (name, value))
        .collect()
}

/// Values by name, each marked when it is in a language other than the
/// default one and may still be replaced.
#[derive(Debug, Default)]
struct Properties(Vec<(&'static str, String, bool)>);

impl Properties {
    fn add(&mut self, name: &'static str, value: &str, list: bool, lang: Option<&str>) {
        let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
        if value.is_empty() {
            return;
        }
        let default = lang.is_none_or(|l| l.eq_ignore_ascii_case("x-default"));
        match self.0.iter_mut().find(|(n, ..)| *n == name) {
            None => self.0.push((name, value, !default)),
            Some((_, existing, _)) if list => {
                if !existing.split("; ").any(|v| v == value) {
                    existing.push_str("; ");
                    existing.push_str(&value);
                }
            }
            Some((_, existing, tentative)) if *tentative && default => {
                *existing = value;
                *tentative = false;
            }
            Some(_) => {}
        }
    }

    /// Properties written as attributes of `rdf:Description`.
    fn attributes(&mut self, e: &BytesStart<'_>) {
        for a in e.attributes().flatten() {
            if let Some((name, list)) = property(a.key.local_name().as_ref())
                && let Ok(value) = a.normalized_value(XmlVersion::Implicit1_0)
            {
                self.add(name, &value, list, None);
            }
        }
    }
}

/// The doctopus metadata name of an XMP property, and whether its values
/// form a list.
fn property(local: &str) -> Option<(&'static str, bool)> {
    Some(match local {
        "title" => ("title", false),
        "creator" => ("author", true),
        "description" => ("subject", false),
        "subject" | "Keywords" => ("keywords", true),
        "CreatorTool" => ("application", false),
        "Producer" => ("producer", false),
        "CreateDate" => ("created", false),
        "ModifyDate" => ("modified", false),
        _ => return None,
    })
}

fn xml_lang(e: &BytesStart<'_>) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == "xml:lang")
        .and_then(|a| {
            a.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|v| v.into_owned())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn properties_come_from_elements_and_attributes() {
        let packet = br#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:pdf="http://ns.adobe.com/pdf/1.3/" pdf:Producer="Writer &amp; Co" pdf:Keywords="a, b"/>
<rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:xmp="http://ns.adobe.com/xap/1.0/">
<dc:title><rdf:Alt><rdf:li xml:lang="de">Titel</rdf:li><rdf:li xml:lang="x-default">Title</rdf:li></rdf:Alt></dc:title>
<dc:creator><rdf:Seq><rdf:li>Ann</rdf:li><rdf:li>Bob</rdf:li></rdf:Seq></dc:creator>
<xmp:CreateDate>2020-01-02T03:04:05Z</xmp:CreateDate>
</rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;
        assert_eq!(
            properties(packet),
            [
                ("producer", "Writer & Co".to_string()),
                ("keywords", "a, b".to_string()),
                ("title", "Title".to_string()),
                ("author", "Ann; Bob".to_string()),
                ("created", "2020-01-02T03:04:05Z".to_string()),
            ]
        );
    }

    #[test]
    fn damaged_packets_yield_what_was_read() {
        assert!(properties(b"<x:xmpmeta><rdf:RDF><dc:title>").is_empty());
        assert!(properties(b"\xff\xfe garbage").is_empty());
    }
}
