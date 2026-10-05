// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Rendering: compact tables by default, `-o json|yaml|wide|name`.

use super::kinds::{Column, KindDef};
use anyhow::Result;
use chrono::{DateTime, Utc};
use clap::ValueEnum;
use comfy_table::{presets, CellAlignment, ContentArrangement, Table};
use serde_json::Value;
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Default)]
pub enum Output {
    #[default]
    Table,
    Wide,
    Json,
    Yaml,
    Name,
}

#[derive(Clone, Copy, Debug)]
pub struct Format {
    pub output: Output,
    pub no_headers: bool,
}

/// `mach-0199a5e0-…-5b0403b597fd` -> `mach-03b597fd`. Ids are UUIDv7, so
/// the leading hex is a timestamp shared by everything made in the same
/// millisecond; the tail is the random part that tells them apart.
pub fn short_id(id: &str) -> String {
    match id.split_once('-') {
        Some((kind, uuid)) => {
            let hex: String = uuid.chars().filter(|c| *c != '-').collect();
            let tail = hex.get(hex.len().saturating_sub(8)..).unwrap_or(&hex);
            format!("{kind}-{tail}")
        }
        None => id.to_owned(),
    }
}

/// `3s`, `12m`, `5h`, `9d`.
pub fn age(created: &str, now: DateTime<Utc>) -> String {
    let Ok(t) = DateTime::parse_from_rfc3339(created) else {
        return String::new();
    };
    let secs = (now - t.with_timezone(&Utc)).num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Render a list of resources of one kind.
pub fn print_list(out: &mut dyn Write, fmt: Format, kind: &KindDef, items: &[Value]) -> Result<()> {
    match fmt.output {
        Output::Json => {
            writeln!(out, "{}", serde_json::to_string_pretty(items)?)?;
        }
        Output::Yaml => {
            write!(out, "{}", serde_yaml_ng::to_string(items)?)?;
        }
        Output::Name => {
            for it in items {
                writeln!(out, "{}", str_at(it, "/name"))?;
            }
        }
        Output::Table | Output::Wide => {
            let cols: Vec<&Column> = if fmt.output == Output::Wide {
                kind.columns.iter().chain(kind.wide.iter()).collect()
            } else {
                kind.columns.iter().collect()
            };
            let now = Utc::now();
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|it| cols.iter().map(|c| (c.get)(it, now)).collect())
                .collect();
            let headers: Vec<&str> = cols.iter().map(|c| c.header).collect();
            write!(out, "{}", table(&headers, &rows, fmt.no_headers))?;
        }
    }
    Ok(())
}

/// Render one resource: full document for json/yaml, a FIELD/VALUE table
/// otherwise.
pub fn print_one(out: &mut dyn Write, fmt: Format, item: &Value) -> Result<()> {
    match fmt.output {
        Output::Json => writeln!(out, "{}", serde_json::to_string_pretty(item)?)?,
        Output::Yaml => write!(out, "{}", serde_yaml_ng::to_string(item)?)?,
        Output::Name => writeln!(out, "{}", str_at(item, "/name"))?,
        Output::Table | Output::Wide => {
            let mut rows = Vec::new();
            flatten("", item, &mut rows);
            let rows: Vec<Vec<String>> = rows.into_iter().map(|(k, v)| vec![k, v]).collect();
            write!(out, "{}", table(&["FIELD", "VALUE"], &rows, fmt.no_headers))?;
        }
    }
    Ok(())
}

/// Any JSON value as a table when it is a list of objects, else pretty JSON.
pub fn print_value(out: &mut dyn Write, fmt: Format, v: &Value) -> Result<()> {
    match fmt.output {
        Output::Yaml => write!(out, "{}", serde_yaml_ng::to_string(v)?)?,
        _ => writeln!(out, "{}", serde_json::to_string_pretty(v)?)?,
    }
    Ok(())
}

/// Build a compact table: no borders, two spaces between columns.
pub fn table(headers: &[&str], rows: &[Vec<String>], no_headers: bool) -> String {
    let mut t = Table::new();
    t.load_preset(presets::NOTHING);
    t.set_content_arrangement(ContentArrangement::Disabled);
    if !no_headers {
        t.set_header(headers.iter().map(|h| h.to_uppercase()));
    }
    for r in rows {
        t.add_row(r.clone());
    }
    for (i, col) in t.column_iter_mut().enumerate() {
        col.set_padding((0, if i + 1 == headers.len() { 0 } else { 2 }));
        col.set_cell_alignment(CellAlignment::Left);
    }
    let lines: Vec<String> = t
        .lines()
        .map(|l| l.trim_end().to_owned())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

fn flatten(prefix: &str, v: &Value, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out);
            }
        }
        Value::Array(a) if a.iter().all(|x| !x.is_object() && !x.is_array()) => {
            out.push((prefix.to_owned(), join(a)));
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                flatten(&format!("{prefix}[{i}]"), x, out);
            }
        }
        other => out.push((prefix.to_owned(), scalar(other))),
    }
}

pub fn scalar(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub fn join(a: &[Value]) -> String {
    a.iter().map(scalar).collect::<Vec<_>>().join(",")
}

/// String at a JSON pointer, `""` when missing.
pub fn str_at(v: &Value, pointer: &str) -> String {
    v.pointer(pointer).map(scalar).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::kinds;
    use pistack_types::Page;
    use serde_json::json;

    fn sample() -> Page<Value> {
        let created = (Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        Page {
            items: vec![
                json!({
                    "id": "mach-0199a5e0-1111-7000-8000-000000000001",
                    "name": "web",
                    "project": "proj-0199a5e0-2222-7000-8000-000000000002",
                    "created_at": created,
                    "updated_at": created,
                    "revision": 3,
                    "spec": {"kind": "vm", "size": "m.2x4", "image": "ubuntu", "networks": ["lan"]},
                    "status": {"phase": "ready", "power": "on", "addresses": ["10.1.0.5"]}
                }),
                json!({
                    "id": "mach-0199a5e0-3333-7000-8000-000000000003",
                    "name": "db",
                    "created_at": created,
                    "updated_at": created,
                    "revision": 1,
                    "spec": {"kind": "container", "size": "s.1x1", "image": "postgres"},
                    "status": {"phase": "pending", "power": "unknown"}
                }),
            ],
            next: None,
            total: 2,
        }
    }

    #[test]
    fn machine_table() {
        let page = sample();
        let mut buf = Vec::new();
        let fmt = Format {
            output: Output::Table,
            no_headers: false,
        };
        print_list(
            &mut buf,
            fmt,
            kinds::by_noun("machine").unwrap(),
            &page.items,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 3, "{s}");
        assert!(lines[0].starts_with("NAME  ID"), "{s}");
        assert!(
            lines[0].contains("PHASE") && lines[0].ends_with("AGE"),
            "{s}"
        );
        assert!(lines[1].starts_with("web   mach-00000001  ready"), "{s}");
        assert!(lines[1].contains("m.2x4") && lines[1].contains("10.1.0.5"));
        assert!(lines[1].trim_end().ends_with("5m"), "{s}");
        assert!(lines[2].starts_with("db    mach-00000003  pending"), "{s}");
        // No trailing whitespace, no border characters.
        assert!(!s.contains('|') && !s.contains('+'));
        assert!(s.lines().all(|l| l == l.trim_end()));
    }

    #[test]
    fn no_headers_and_name_output() {
        let page = sample();
        let mut buf = Vec::new();
        let fmt = Format {
            output: Output::Table,
            no_headers: true,
        };
        print_list(
            &mut buf,
            fmt,
            kinds::by_noun("machine").unwrap(),
            &page.items,
        )
        .unwrap();
        assert_eq!(String::from_utf8(buf).unwrap().lines().count(), 2);

        let mut buf = Vec::new();
        let fmt = Format {
            output: Output::Name,
            no_headers: false,
        };
        print_list(
            &mut buf,
            fmt,
            kinds::by_noun("machine").unwrap(),
            &page.items,
        )
        .unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "web\ndb\n");
    }

    #[test]
    fn wide_adds_project_and_full_id() {
        let page = sample();
        let mut buf = Vec::new();
        let fmt = Format {
            output: Output::Wide,
            no_headers: false,
        };
        print_list(
            &mut buf,
            fmt,
            kinds::by_noun("machine").unwrap(),
            &page.items,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("PROJECT"));
        assert!(s.contains("mach-0199a5e0-1111-7000-8000-000000000001"));
    }

    #[test]
    fn json_and_yaml_roundtrip() {
        let page = sample();
        let mut buf = Vec::new();
        let fmt = Format {
            output: Output::Json,
            no_headers: false,
        };
        print_list(
            &mut buf,
            fmt,
            kinds::by_noun("machine").unwrap(),
            &page.items,
        )
        .unwrap();
        let back: Vec<Value> = serde_json::from_slice(&buf).unwrap();
        assert_eq!(back, page.items);

        let mut buf = Vec::new();
        print_one(
            &mut buf,
            Format {
                output: Output::Yaml,
                no_headers: false,
            },
            &page.items[0],
        )
        .unwrap();
        let back: Value = serde_yaml_ng::from_slice(&buf).unwrap();
        assert_eq!(back["spec"]["size"], "m.2x4");
    }

    #[test]
    fn show_flattens() {
        let page = sample();
        let mut buf = Vec::new();
        print_one(
            &mut buf,
            Format {
                output: Output::Table,
                no_headers: false,
            },
            &page.items[0],
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("spec.networks"), "{s}");
        assert!(s.contains("status.addresses  10.1.0.5"), "{s}");
    }

    #[test]
    fn helpers() {
        assert_eq!(
            short_id("mach-0199a5e0-1111-7000-8000-00000000abcd"),
            "mach-0000abcd"
        );
        assert_eq!(short_id("plain"), "plain");
        let now = Utc::now();
        assert_eq!(
            age(&(now - chrono::Duration::seconds(5)).to_rfc3339(), now),
            "5s"
        );
        assert_eq!(
            age(&(now - chrono::Duration::hours(3)).to_rfc3339(), now),
            "3h"
        );
        assert_eq!(
            age(&(now - chrono::Duration::days(2)).to_rfc3339(), now),
            "2d"
        );
        assert_eq!(age("garbage", now), "");
    }

    #[test]
    fn volume_list_shows_attaching_and_the_host_path() {
        let created = Utc::now().to_rfc3339();
        let item = json!({
            "name": "data",
            "id": "vol-0199a5e0-1111-7000-8000-000000000001",
            "created_at": created,
            "spec": {"size_gib": 1, "volume_type": "standard", "bootable": false},
            "status": {
                "phase": "ready",
                "attached_to": "mach-0199a5e0-2222-7000-8000-000000000002",
                "device": "vdb",
                "host_path": "/var/lib/pistack/volumes/data",
                "conditions": [{
                    "type": "Attached",
                    "status": false,
                    "reason": "Attaching",
                    "message": "waiting",
                    "last_transition": created
                }]
            }
        });
        let mut buf = Vec::new();
        print_list(
            &mut buf,
            Format {
                output: Output::Table,
                no_headers: false,
            },
            kinds::by_noun("volume").unwrap(),
            std::slice::from_ref(&item),
        )
        .unwrap();
        let listed = String::from_utf8(buf).unwrap();
        assert!(listed.contains("ATTACH"), "{listed}");
        assert!(listed.contains("HOST-PATH"), "{listed}");
        assert!(listed.contains("Attaching"), "{listed}");
        assert!(listed.contains("/var/lib/pistack/volumes/data"), "{listed}");

        let mut buf = Vec::new();
        print_one(
            &mut buf,
            Format {
                output: Output::Table,
                no_headers: false,
            },
            &item,
        )
        .unwrap();
        let shown = String::from_utf8(buf).unwrap();
        assert!(shown.contains("status.host_path"), "{shown}");
        assert!(shown.contains("/var/lib/pistack/volumes/data"), "{shown}");
        assert!(shown.contains("Attaching"), "{shown}");
    }
}
