// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Default minimum size of every part except the last (5 MiB, S3 rule).
pub const MIN_PART_BYTES: usize = 5 * 1024 * 1024;
