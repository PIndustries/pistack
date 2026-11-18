// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

use super::marker_detail::MARKER_DETAIL;

/// The marker version an error names, when it names one.
///
/// [`hidden`] and [`not_a_readable_object`] put it there, so the route can
/// answer the two marker headers off the error the read already produced
/// rather than reading the store a second time.
pub(crate) fn marker_version_of(err: &ApiError) -> Option<u64> {
    err.details
        .get(MARKER_DETAIL)
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok())
}
