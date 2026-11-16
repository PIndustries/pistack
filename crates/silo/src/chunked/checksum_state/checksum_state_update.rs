// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use sha2::Digest;

use crate::chunked::checksum_state::checksum_state::ChecksumState;
use crate::chunked::checksum_state::trailer_checksum::TrailerChecksum;

impl ChecksumState {
    pub(in crate::chunked) fn update(&mut self, data: &[u8]) {
        match self.algo {
            TrailerChecksum::Crc32 => self.crc32.update(data),
            TrailerChecksum::Crc32c => {
                self.crc32c = crc32c::crc32c_append(self.crc32c, data);
            }
            TrailerChecksum::Sha256 => self.sha256.update(data),
            TrailerChecksum::Sha1 => self.sha1.update(data),
        }
    }
}
