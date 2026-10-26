//! List numbering: list definitions, the counters that run through a
//! document, and their labels such as `3.`, `c)` or `iv.`. Shared by the
//! Word readers (both formats keep lists the same way) and PowerPoint.

use std::collections::{HashMap, HashSet};

/// How a list counter is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CounterStyle {
    Decimal,
    /// Decimal padded to two digits (`01`).
    DecimalZero,
    LowerLetter,
    UpperLetter,
    LowerRoman,
    UpperRoman,
    /// Not numbered: the label is a bullet character.
    Bullet,
    /// Numbered, but not displayed (`w:numFmt="none"`).
    None,
}

impl CounterStyle {
    /// Map a WordprocessingML `w:numFmt` value.
    pub(crate) fn from_word(fmt: &str) -> Self {
        match fmt {
            "decimalZero" => Self::DecimalZero,
            "lowerLetter" => Self::LowerLetter,
            "upperLetter" => Self::UpperLetter,
            "lowerRoman" => Self::LowerRoman,
            "upperRoman" => Self::UpperRoman,
            "bullet" => Self::Bullet,
            "none" => Self::None,
            // decimal, ordinal, cardinalText, and the many East Asian
            // counters all fall back to plain numbers.
            _ => Self::Decimal,
        }
    }

    /// Map a number format code of the binary Word format (`nfc`).
    pub(crate) fn from_nfc(nfc: u8) -> Self {
        match nfc {
            1 => Self::UpperRoman,
            2 => Self::LowerRoman,
            3 => Self::UpperLetter,
            4 => Self::LowerLetter,
            22 => Self::DecimalZero,
            23 => Self::Bullet,
            255 => Self::None,
            _ => Self::Decimal,
        }
    }

    /// Format a counter value.
    pub(crate) fn format(self, n: u32) -> String {
        match self {
            Self::Decimal => n.to_string(),
            Self::DecimalZero => format!("{n:02}"),
            Self::LowerLetter => letters(n),
            Self::UpperLetter => letters(n).to_uppercase(),
            Self::LowerRoman => roman(n),
            Self::UpperRoman => roman(n).to_uppercase(),
            Self::Bullet | Self::None => String::new(),
        }
    }
}

/// DrawingML `a:buAutoNum` schemes (`arabicPeriod`, `alphaLcParenR`, ...),
/// as a counter style plus the text around the number.
pub(crate) fn auto_number_scheme(scheme: &str) -> (CounterStyle, &'static str, &'static str) {
    let style = if scheme.starts_with("alphaLc") {
        CounterStyle::LowerLetter
    } else if scheme.starts_with("alphaUc") {
        CounterStyle::UpperLetter
    } else if scheme.starts_with("romanLc") {
        CounterStyle::LowerRoman
    } else if scheme.starts_with("romanUc") {
        CounterStyle::UpperRoman
    } else {
        CounterStyle::Decimal
    };
    let (before, after) = if scheme.ends_with("ParenBoth") {
        ("(", ")")
    } else if scheme.ends_with("ParenR") {
        ("", ")")
    } else if scheme.ends_with("Plain") {
        ("", "")
    } else if scheme.ends_with("Minus") {
        ("- ", " -")
    } else {
        ("", ".")
    };
    (style, before, after)
}

/// `a`..`z`, then `aa`, `bb`, ... as Word does.
fn letters(n: u32) -> String {
    if n == 0 {
        return "0".into();
    }
    let letter = char::from(b'a' + ((n - 1) % 26) as u8);
    let repeat = ((n - 1) / 26 + 1) as usize;
    std::iter::repeat_n(letter, repeat.min(10)).collect()
}

fn roman(mut n: u32) -> String {
    if n == 0 || n >= 4000 {
        return n.to_string();
    }
    const TABLE: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (value, digits) in TABLE {
        while n >= value {
            out.push_str(digits);
            n -= value;
        }
    }
    out
}

/// A reference to a list: numbering instance and level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NumRef {
    pub(crate) num_id: String,
    pub(crate) level: Option<u8>,
}

/// One level of a list definition.
#[derive(Debug, Clone)]
pub(crate) struct Level {
    pub(crate) start: u32,
    pub(crate) style: CounterStyle,
    /// The label template: `%1.%2)` stands for the counters of levels 1
    /// and 2.
    pub(crate) text: String,
}

impl Default for Level {
    fn default() -> Self {
        Self {
            start: 1,
            style: CounterStyle::Decimal,
            text: String::new(),
        }
    }
}

/// A list instance: a definition plus per-level overrides.
#[derive(Debug, Default)]
pub(crate) struct Instance {
    pub(crate) abstract_id: String,
    pub(crate) start_overrides: HashMap<u8, u32>,
    pub(crate) level_overrides: HashMap<u8, Level>,
}

/// List definitions and their instances.
#[derive(Debug, Default)]
pub(crate) struct Numbering {
    /// Instances by id (`w:numId`; the 1-based LFO index in `.doc` files).
    pub(crate) instances: HashMap<String, Instance>,
    /// Definitions by id, nine levels each.
    pub(crate) abstracts: HashMap<String, Vec<Level>>,
}

impl Numbering {
    fn level(&self, num_id: &str, level: u8) -> Option<(&str, Level)> {
        let instance = self.instances.get(num_id)?;
        let mut def = instance.level_overrides.get(&level).cloned().or_else(|| {
            self.abstracts
                .get(&instance.abstract_id)?
                .get(usize::from(level))
                .cloned()
        })?;
        if let Some(start) = instance.start_overrides.get(&level) {
            def.start = *start;
        }
        Some((instance.abstract_id.as_str(), def))
    }
}

/// Running list counters.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    /// Counter values per abstract list, indexed by level (`None` until
    /// the level is used, or after a higher level moved on).
    values: HashMap<String, [Option<u32>; 9]>,
    /// Instances whose start overrides have been applied, by level.
    started: HashSet<(String, u8)>,
}

/// The outcome of numbering a paragraph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ListLabel {
    /// Not in a list.
    None,
    /// A bulleted (or unlabelled) list item.
    Bullet,
    /// A numbered item with its label (`3.`, `2.1`, `(b)`).
    Number(String),
}

impl Counters {
    /// Advance the counter for a list paragraph and return its label.
    pub(crate) fn next(&mut self, numbering: &Numbering, num: &NumRef) -> ListLabel {
        let level = num.level.unwrap_or(0).min(8);
        let Some((abstract_id, def)) = numbering.level(&num.num_id, level) else {
            return ListLabel::None;
        };
        let abstract_id = abstract_id.to_string();
        let values = self.values.entry(abstract_id).or_insert([None; 9]);
        let i = usize::from(level);
        if self.started.insert((num.num_id.clone(), level))
            && let Some(instance) = numbering.instances.get(&num.num_id)
            && instance.start_overrides.contains_key(&level)
        {
            values[i] = None;
        }
        values[i] = Some(values[i].map_or(def.start, |v| v.saturating_add(1)));
        for deeper in values.iter_mut().skip(i + 1) {
            *deeper = None;
        }
        if def.style == CounterStyle::Bullet {
            return ListLabel::Bullet;
        }
        if def.style == CounterStyle::None || def.text.is_empty() {
            return ListLabel::Bullet;
        }
        let snapshot = *values;
        let mut label = String::new();
        let mut chars = def.text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%'
                && let Some(d) = chars.peek().and_then(|d| d.to_digit(10))
            {
                chars.next();
                let at = (d as usize).saturating_sub(1).min(8);
                let (style, start) = numbering
                    .level(&num.num_id, at as u8)
                    .map_or((CounterStyle::Decimal, 1), |(_, l)| (l.style, l.start));
                let value = snapshot[at].unwrap_or(start);
                label.push_str(&style.format(value));
                continue;
            }
            label.push(c);
        }
        let label = label.trim().to_string();
        if label.is_empty() {
            ListLabel::Bullet
        } else {
            ListLabel::Number(label)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_counters() {
        assert_eq!(CounterStyle::Decimal.format(12), "12");
        assert_eq!(CounterStyle::DecimalZero.format(3), "03");
        assert_eq!(CounterStyle::LowerLetter.format(28), "bb");
        assert_eq!(CounterStyle::UpperRoman.format(1994), "MCMXCIV");
    }

    #[test]
    fn parses_auto_number_schemes() {
        assert_eq!(
            auto_number_scheme("arabicPeriod"),
            (CounterStyle::Decimal, "", ".")
        );
        assert_eq!(
            auto_number_scheme("alphaLcParenBoth"),
            (CounterStyle::LowerLetter, "(", ")")
        );
        assert_eq!(
            auto_number_scheme("romanUcParenR"),
            (CounterStyle::UpperRoman, "", ")")
        );
    }
}
