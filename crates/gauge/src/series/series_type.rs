// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::escape::escape;
use super::sanitize::{sanitize_label, sanitize_name};

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
