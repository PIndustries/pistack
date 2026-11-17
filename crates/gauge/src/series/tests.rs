// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

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
