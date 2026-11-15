// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::streaming_auth::streaming_kind::StreamingKind;

/// Seed material from a verified SigV4 request, needed to decode the body.
#[derive(Clone, Debug)]
pub struct StreamingAuth {
    pub kind: StreamingKind,
    pub seed_signature: String,
    pub signing_key: Vec<u8>,
    pub amz_date: String,
    pub scope: String,
    pub decoded_content_length: u64,
}
