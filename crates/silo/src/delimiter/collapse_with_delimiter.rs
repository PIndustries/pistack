// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::common_prefix_after::common_prefix_after;
use super::delimited::Delimited;

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
