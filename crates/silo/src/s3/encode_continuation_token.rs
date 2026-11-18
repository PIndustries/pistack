// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

/// URL-safe base64 (no padding) of the resume key for ListObjectsV2.
pub(crate) fn encode_continuation_token(key: &str) -> String {
    URL_SAFE_NO_PAD.encode(key.as_bytes())
}
