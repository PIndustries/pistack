// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::parse_limit::parse_limit;
use super::s3_max_keys::S3_MAX_KEYS;

pub(crate) fn parse_max_keys(raw: Option<&str>) -> ApiResult<usize> {
    parse_limit(raw, "max-keys", S3_MAX_KEYS)
}
