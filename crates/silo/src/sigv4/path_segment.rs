// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC};

/// Characters left unescaped in an S3 SigV4 path segment (AWS rules).
/// Encode everything in NON_ALPHANUMERIC except `-`, `_`, `.`, `~`.
pub(crate) const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');
