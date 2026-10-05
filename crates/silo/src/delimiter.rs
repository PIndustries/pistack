// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3-style `delimiter` / `CommonPrefixes` helpers.
//!
//! Shared by ListObjectsV2 and ListMultipartUploads so both APIs roll up
//! keys the same way. Resuming after a common-prefix marker skips every
//! key under that prefix so the next page neither duplicates nor skips it.

/// After `prefix`, if `delimiter` appears in the remainder, return the
/// rolled-up common prefix (including the delimiter).
///
/// If a key equals the common prefix string (e.g. `a/` with delimiter `/`),
/// S3 would group it into that CommonPrefix; silo currently rejects
/// trailing-slash object keys, so this only matters for the string helper.
pub(crate) fn common_prefix_after(key: &str, prefix: &str, delimiter: &str) -> Option<String> {
    if delimiter.is_empty() || !key.starts_with(prefix) {
        return None;
    }
    let rest = &key[prefix.len()..];
    rest.find(delimiter).map(|i| {
        let end = i + delimiter.len();
        format!("{}{}", prefix, &rest[..end])
    })
}

/// True when `after` is a delimiter-rolled common-prefix marker (non-empty
/// and ends with `delimiter`). Resuming after such a marker must skip
/// every key under that prefix so the next page neither duplicates nor
/// skips the CommonPrefix.
pub(crate) fn is_common_prefix_marker(after: &str, delimiter: &str) -> bool {
    !delimiter.is_empty() && !after.is_empty() && after.ends_with(delimiter)
}

/// Exclusive start-after for a key under optional delimiter grouping.
pub(crate) fn key_after_marker(key: &str, after: &str, delimiter: Option<&str>) -> bool {
    match delimiter {
        Some(d) if is_common_prefix_marker(after, d) => key > after && !key.starts_with(after),
        _ => key > after,
    }
}

/// One collapsed row: either an original item or a rolled-up prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Delimited<T> {
    Item(T),
    CommonPrefix(String),
}

/// Collapse a **sorted** sequence of items by S3 delimiter.
///
/// Items whose key rolls up to the same common prefix emit one
/// [`Delimited::CommonPrefix`]; others emit [`Delimited::Item`] in order
/// (so multiple uploads for the same non-rolled key are all kept).
pub(crate) fn collapse_with_delimiter<T>(
    items: impl IntoIterator<Item = T>,
    prefix: &str,
    delimiter: &str,
    mut key_of: impl FnMut(&T) -> &str,
) -> Vec<Delimited<T>> {
    let mut out = Vec::new();
    let mut last_cp: Option<String> = None;
    for item in items {
        match common_prefix_after(key_of(&item), prefix, delimiter) {
            Some(cp) => {
                if last_cp.as_ref() != Some(&cp) {
                    out.push(Delimited::CommonPrefix(cp.clone()));
                    last_cp = Some(cp);
                }
            }
            None => {
                last_cp = None;
                out.push(Delimited::Item(item));
            }
        }
    }
    out
}

/// Whether an upload survives key-marker / upload-id-marker under optional
/// delimiter (filter **before** collapse so a common-prefix marker does not
/// reappear on the next page).
pub(crate) fn upload_after_markers(
    key: &str,
    upload_id: &str,
    key_marker: Option<&str>,
    upload_id_marker: Option<&str>,
    delimiter: Option<&str>,
) -> bool {
    let Some(km) = key_marker else {
        return true;
    };
    if delimiter.is_some_and(|d| is_common_prefix_marker(km, d)) {
        return key_after_marker(key, km, delimiter);
    }
    match upload_id_marker {
        None => key > km,
        Some(um) => key > km || (key == km && upload_id > um),
    }
}

#[cfg(test)]
mod tests {
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
}
