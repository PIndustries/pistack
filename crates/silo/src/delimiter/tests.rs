// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::*;

#[test]
fn key_equal_to_common_prefix_is_grouped() {
    // String-level: if such a key existed, it would roll up into the CP.
    assert_eq!(common_prefix_after("a/", "", "/"), Some("a/".to_owned()));
    assert_eq!(common_prefix_after("a/1", "", "/"), Some("a/".to_owned()));
    assert_eq!(common_prefix_after("a", "", "/"), None);
    assert_eq!(
        common_prefix_after("logs/a/", "logs/", "/"),
        Some("logs/a/".to_owned())
    );
}

#[test]
fn resume_after_common_prefix_skips_under_prefix() {
    let d = Some("/");
    assert!(!key_after_marker("a/", "a/", d));
    assert!(!key_after_marker("a/1", "a/", d));
    assert!(!key_after_marker("a/z", "a/", d));
    assert!(key_after_marker("b", "a/", d));
    assert!(key_after_marker("b/1", "a/", d));
    // Non-CP marker: exclusive string compare only.
    assert!(!key_after_marker("readme", "readme", d));
    assert!(key_after_marker("readme2", "readme", d));
    assert!(key_after_marker("a/1", "a", d)); // "a/1" > "a"
}

#[test]
fn collapse_groups_key_equal_to_prefix_once() {
    let keys = ["a/", "a/1", "a/2", "b", "c/x"];
    let entries = collapse_with_delimiter(keys, "", "/", |k| *k);
    assert_eq!(
        entries,
        vec![
            Delimited::CommonPrefix("a/".into()),
            Delimited::Item("b"),
            Delimited::CommonPrefix("c/".into()),
        ]
    );
}

#[test]
fn collapse_then_page_after_cp_no_duplicate() {
    let keys = ["a/", "a/1", "b", "c/x"];
    let all = collapse_with_delimiter(keys, "", "/", |k| *k);
    assert_eq!(all[0], Delimited::CommonPrefix("a/".into()));
    // Simulate max-keys=1 ending on CP a/, then resume.
    let rest: Vec<_> = keys
        .into_iter()
        .filter(|k| key_after_marker(k, "a/", Some("/")))
        .collect();
    let page2 = collapse_with_delimiter(rest, "", "/", |k| *k);
    assert_eq!(
        page2,
        vec![Delimited::Item("b"), Delimited::CommonPrefix("c/".into()),]
    );
}
