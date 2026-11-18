// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Max clock skew accepted between `x-amz-date` / `X-Amz-Date` and server time.
pub const MAX_SKEW_SECS: i64 = 15 * 60;
