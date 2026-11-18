// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::get_range::GetRange;

/// The range clipped to an object of `size` bytes, as `(first, last)` with
/// both included, or `None` when the object cannot answer it and S3 answers
/// 416: a first byte at or past the end of a non-empty object, or a suffix of
/// zero bytes.
///
/// On an empty object nothing is unsatisfiable — every range asks for bytes
/// that are simply not there, and the answer is the empty object.
pub fn satisfiable(range: GetRange, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return Some((0, 0));
    }
    match range {
        GetRange::From { first, last } => {
            if first >= size {
                return None;
            }
            Some((first, last.min(size - 1)))
        }
        GetRange::Suffix { length } => {
            if length == 0 {
                return None;
            }
            Some((size.saturating_sub(length), size - 1))
        }
    }
}
