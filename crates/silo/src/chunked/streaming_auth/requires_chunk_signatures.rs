// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::streaming_auth::streaming_auth::StreamingAuth;
use crate::chunked::streaming_auth::streaming_kind::StreamingKind;

impl StreamingAuth {
    pub fn requires_chunk_signatures(&self) -> bool {
        matches!(
            self.kind,
            StreamingKind::Signed | StreamingKind::SignedTrailer { .. }
        )
    }
}
