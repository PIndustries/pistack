// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::trailer_checksum::TrailerChecksum;

/// How the framed body is authenticated.
#[derive(Clone, Debug)]
pub enum StreamingKind {
    Signed,
    UnsignedTrailer { checksum: TrailerChecksum },
    SignedTrailer { checksum: TrailerChecksum },
}
