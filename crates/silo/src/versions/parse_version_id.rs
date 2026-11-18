// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::no_such_version::no_such_version;

/// A `versionId` as sent on the wire: a generation number in decimal.
pub(crate) fn parse_version_id(raw: &str) -> ApiResult<u64> {
    raw.trim().parse::<u64>().map_err(|_| no_such_version(raw))
}
