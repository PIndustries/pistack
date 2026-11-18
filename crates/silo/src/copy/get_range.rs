// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// What a `Range` header asked for. `None` means the header is not a single
/// satisfiable byte range and the whole object is served, which is what S3
/// does with a multi-range or malformed header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GetRange {
    /// `bytes=a-b` or `bytes=a-`: from `first` to `last`, both included.
    From { first: u64, last: u64 },
    /// `bytes=-n`: the last `n` bytes.
    Suffix { length: u64 },
}
