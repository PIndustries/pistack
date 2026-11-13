// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A series is a metric name and a set of labels, written the way
//! Prometheus writes it: `pistack_machine_memory_bytes{host="h1",machine="mach-1"}`.
//! That text is the key samples are stored under.

use std::collections::BTreeMap;
use std::fmt::Write as _;

/// A metric name and its labels.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Series {
    pub name: String,
    pub labels: BTreeMap<String, String>,
}

impl Series {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            labels: BTreeMap::new(),
        }
    }

    pub fn label(mut self, key: &str, value: impl Into<String>) -> Self {
        self.labels.insert(key.to_owned(), value.into());
        self
    }

    /// Read `name` or `name{k="v",...}`. Text that is not in that form is
    /// taken as far as it reads: a broken label list keeps the labels before
    /// the break.
    pub fn parse(text: &str) -> Series {
        let text = text.trim();
        let Some((name, rest)) = text.split_once('{') else {
            return Series::new(text);
        };
        let mut series = Series::new(name.trim());
        let mut chars = rest.chars().peekable();
        loop {
            while chars.peek().is_some_and(|c| *c == ',' || c.is_whitespace()) {
                chars.next();
            }
            let mut key = String::new();
            for c in chars.by_ref() {
                if c == '=' || c == '}' {
                    break;
                }
                key.push(c);
            }
            let key = key.trim().to_owned();
            if key.is_empty() || chars.next() != Some('"') {
                break;
            }
            let mut value = String::new();
            let mut closed = false;
            while let Some(c) = chars.next() {
                match c {
                    '"' => {
                        closed = true;
                        break;
                    }
                    '\\' => match chars.next() {
                        Some('n') => value.push('\n'),
                        Some(other) => value.push(other),
                        None => break,
                    },
                    other => value.push(other),
                }
            }
            if !closed {
                break;
            }
            series.labels.insert(key, value);
        }
        series
    }

    /// The text this series is stored under: labels in name order, values
    /// escaped. The name is kept as it is.
    pub fn key(&self) -> String {
        self.render(&self.name, false)
    }

    /// The series as a line of the Prometheus text format starts: the name
    /// and the label names made legal, values escaped.
    pub fn exposition(&self) -> String {
        self.render(&sanitize_name(&self.name), true)
    }

    fn render(&self, name: &str, sanitize: bool) -> String {
        let mut out = name.to_owned();
        if self.labels.is_empty() {
            return out;
        }
        out.push('{');
        for (i, (key, value)) in self.labels.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            if sanitize {
                out.push_str(&sanitize_label(key));
            } else {
                out.push_str(key);
            }
            let _ = write!(out, "=\"{}\"", escape(value));
        }
        out.push('}');
        out
    }

    /// True when `other` has this name and every label this one has, with
    /// the same value. A series with no labels selects every series of its
    /// name.
    pub fn selects(&self, other: &Series) -> bool {
        self.name == other.name
            && self
                .labels
                .iter()
                .all(|(key, value)| other.labels.get(key) == Some(value))
    }
}

/// The key for `name` with `labels`.
pub fn series(name: &str, labels: &[(&str, &str)]) -> String {
    let mut s = Series::new(name);
    for (key, value) in labels {
        s.labels.insert((*key).to_owned(), (*value).to_owned());
    }
    s.key()
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c if c.is_control() => out.push('_'),
            c => out.push(c),
        }
    }
    out
}

/// Prometheus metric names are `[a-zA-Z_:][a-zA-Z0-9_:]*`. Any other
/// character becomes `_`, and a leading digit gets one in front.
pub fn sanitize_name(name: &str) -> String {
    sanitize(name, true)
}

/// Label names are the same without the colon.
fn sanitize_label(name: &str) -> String {
    sanitize(name, false)
}

fn sanitize(name: &str, colon: bool) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || (colon && c == ':') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        return "_".into();
    }
    if out.as_bytes()[0].is_ascii_digit() {
        out.insert(0, '_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_reads_back_as_the_series_it_was_made_from() {
        let key = series(
            "pistack_machine_memory_bytes",
            &[
                ("name", "we\"b\\1\n"),
                ("host", "h1"),
                ("machine", "mach-1"),
            ],
        );
        assert_eq!(
            key,
            "pistack_machine_memory_bytes{host=\"h1\",machine=\"mach-1\",name=\"we\\\"b\\\\1\\n\"}"
        );
        let back = Series::parse(&key);
        assert_eq!(back.name, "pistack_machine_memory_bytes");
        assert_eq!(back.labels["name"], "we\"b\\1\n");
        assert_eq!(back.labels.len(), 3);
        assert_eq!(back.key(), key);
    }

    #[test]
    fn odd_text_is_read_as_far_as_it_goes() {
        assert_eq!(Series::parse(" cpu.usage "), Series::new("cpu.usage"));
        assert_eq!(Series::parse("m{}"), Series::new("m"));
        let s = Series::parse("m{a=\"1\", b=\"2\"");
        assert_eq!(s.labels.len(), 2);
        let s = Series::parse("m{a=\"1\",b=oops}");
        assert_eq!(s.labels.len(), 1);
        let s = Series::parse("m{a=\"never closed}");
        assert!(s.labels.is_empty());
    }

    #[test]
    fn a_selector_matches_on_the_labels_it_names() {
        let stored = Series::parse("m{host=\"h1\",machine=\"mach-1\"}");
        assert!(Series::parse("m").selects(&stored));
        assert!(Series::parse("m{machine=\"mach-1\"}").selects(&stored));
        assert!(!Series::parse("m{machine=\"mach-2\"}").selects(&stored));
        assert!(!Series::parse("m{zone=\"a\"}").selects(&stored));
        assert!(!Series::parse("other").selects(&stored));
    }

    #[test]
    fn names_are_made_legal_for_the_text_format() {
        let s = Series::new("cpu.usage").label("data-dir", "/var");
        assert_eq!(s.exposition(), "cpu_usage{data_dir=\"/var\"}");
        assert_eq!(sanitize_name("9lives"), "_9lives");
        assert_eq!(sanitize_name(""), "_");
        assert_eq!(sanitize_name("a:b"), "a:b");
    }
}
