//! What font names say about text.

/// Whether a font is fixed-width, the usual sign of code, file names and
/// commands. Goes by the name: `Consolas`, `Courier New`, `Menlo`, and
/// anything with "Mono" or "Code" in it.
pub(crate) fn is_monospace(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    ["mono", "code", "courier", "consol", "typewriter"]
        .iter()
        .any(|word| name.contains(word))
        || [
            "menlo",
            "monaco",
            "inconsolata",
            "hack",
            "fixedsys",
            "terminal",
        ]
        .contains(&name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_width_fonts_are_known_by_name() {
        for font in [
            "Consolas",
            "Courier New",
            "Lucida Console",
            "JetBrains Mono",
            "Menlo",
        ] {
            assert!(is_monospace(font), "{font}");
        }
        for font in [
            "Calibri",
            "Times New Roman",
            "Arial",
            "Segoe UI",
            "+mn-lt",
            "",
        ] {
            assert!(!is_monospace(font), "{font}");
        }
    }
}
