// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::get_range::GetRange;

/// Parse a `Range` header for GetObject. Unlike
/// [`parse_copy_source_range`], a header this cannot read is not an error:
/// it answers `None` and the caller serves the whole object.
///
/// `bytes=a-b`, `bytes=a-` and `bytes=-n` are understood, with `n` of zero
/// or more. `a > b` is malformed, not unsatisfiable — S3 answers 200 with
/// the whole object for it.
pub fn parse_get_range(raw: &str) -> Option<GetRange> {
    let rest = raw.trim().strip_prefix("bytes=")?;
    if rest.contains(',') || !rest.contains('-') {
        return None;
    }
    let (first_s, last_s) = rest.split_once('-')?;
    let first_s = first_s.trim();
    let last_s = last_s.trim();
    if first_s.is_empty() {
        if last_s.is_empty() {
            return None;
        }
        return Some(GetRange::Suffix {
            length: last_s.parse::<u64>().ok()?,
        });
    }
    let first: u64 = first_s.parse().ok()?;
    if last_s.is_empty() {
        return Some(GetRange::From {
            first,
            last: u64::MAX,
        });
    }
    let last: u64 = last_s.parse().ok()?;
    if first > last {
        return None;
    }
    Some(GetRange::From { first, last })
}
