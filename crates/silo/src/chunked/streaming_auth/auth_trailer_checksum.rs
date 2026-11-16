// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::trailer_checksum::TrailerChecksum;
use crate::chunked::streaming_auth::streaming_auth::StreamingAuth;
use crate::chunked::streaming_auth::streaming_kind::StreamingKind;

impl StreamingAuth {
    pub fn trailer_checksum(&self) -> Option<TrailerChecksum> {
        match self.kind {
            StreamingKind::Signed => None,
            StreamingKind::UnsignedTrailer { checksum }
            | StreamingKind::SignedTrailer { checksum } => Some(checksum),
        }
    }
}
