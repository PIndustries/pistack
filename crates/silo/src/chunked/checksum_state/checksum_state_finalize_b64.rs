// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::checksum_state::ChecksumState;
use crate::chunked::checksum_state::trailer_checksum::TrailerChecksum;
use base64::Engine;
use sha2::Digest;

impl ChecksumState {
    pub(in crate::chunked) fn finalize_b64(self) -> String {
        let engine = base64::engine::general_purpose::STANDARD;
        match self.algo {
            TrailerChecksum::Crc32 => engine.encode(self.crc32.finalize().to_be_bytes()),
            TrailerChecksum::Crc32c => engine.encode(self.crc32c.to_be_bytes()),
            TrailerChecksum::Sha256 => engine.encode(self.sha256.finalize().as_slice()),
            TrailerChecksum::Sha1 => engine.encode(self.sha1.finalize().as_slice()),
        }
    }
}
