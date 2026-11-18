// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC};

/// Query-string encoding set for SigV4 (same as path, plus encode `/`).
pub(crate) const QUERY_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');
