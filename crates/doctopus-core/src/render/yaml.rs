//! Just enough YAML to write readable, valid headers.

/// Format a string as a YAML scalar: plain when that is unambiguous,
/// double-quoted (JSON escaping, which YAML accepts) otherwise.
pub(crate) fn scalar(value: &str) -> String {
    if is_plain_safe(value) {
        value.to_string()
    } else {
        serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
    }
}

/// Format a string as a YAML mapping key: plain when it is a simple name,
/// double-quoted otherwise. Property names come from documents and can hold
/// anything, including `: ` and `#`.
pub(crate) fn key(name: &str) -> String {
    let simple = !name.is_empty()
        && name.trim() == name
        && name.chars().next().is_some_and(char::is_alphabetic)
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || " _-.()/".contains(c));
    if simple {
        name.to_string()
    } else {
        serde_json::to_string(name).unwrap_or_else(|_| "\"\"".into())
    }
}

fn is_plain_safe(v: &str) -> bool {
    let Some(first) = v.chars().next() else {
        return false;
    };
    if v.trim() != v || v.contains(['\n', '\r', '\t']) {
        return false;
    }
    if "-?:,[]{}#&*!|>'\"%@`".contains(first) {
        return false;
    }
    if v.contains(": ") || v.contains(" #") || v.ends_with(':') {
        return false;
    }
    let lower = v.to_ascii_lowercase();
    // Values YAML would read as booleans, nulls or numbers stay strings.
    if matches!(
        lower.as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "~"
    ) {
        return false;
    }
    if v.parse::<f64>().is_ok() {
        return false;
    }
    v.chars().all(|c| !c.is_control())
}

#[cfg(test)]
mod tests {
    use super::{key, scalar};

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(scalar("Quarterly report"), "Quarterly report");
        assert_eq!(scalar("Q3: final"), "\"Q3: final\"");
        assert_eq!(scalar("#1 priority"), "\"#1 priority\"");
        assert_eq!(scalar("2024"), "\"2024\"");
        assert_eq!(scalar("yes"), "\"yes\"");
        assert_eq!(scalar("line\nbreak"), "\"line\\nbreak\"");
        assert_eq!(scalar(""), "\"\"");
    }

    #[test]
    fn keys_are_quoted_unless_simple() {
        assert_eq!(key("custom.Client name"), "custom.Client name");
        assert_eq!(key("Client: ACME"), "\"Client: ACME\"");
        assert_eq!(key("#id"), "\"#id\"");
    }
}
