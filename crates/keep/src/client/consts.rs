// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Largest secret payload accepted (1 MiB).
pub const MAX_PAYLOAD: usize = 1 << 20;
/// Limits on a secret's clear-text metadata, so labels, description and
/// content type cannot carry more than the (sealed) payload is allowed to.
/// Applied the same way on create and update, and to certificates.
pub const MAX_LABELS: usize = 64;
pub const MAX_LABEL_KEY: usize = 128;
pub const MAX_LABEL_VALUE: usize = 1024;
pub const MAX_DESCRIPTION: usize = 4096;
pub const MAX_CONTENT_TYPE: usize = 255;
