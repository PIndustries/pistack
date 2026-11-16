// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use sha2::Digest;

use crate::chunked::checksum_state::checksum_state::ChecksumState;
use crate::chunked::checksum_state::trailer_checksum::TrailerChecksum;
use sha1::Sha1;
use sha2::Sha256;

impl ChecksumState {
    pub(in crate::chunked) fn new(algo: TrailerChecksum) -> Self {
        Self {
            algo,
            crc32: crc32fast::Hasher::new(),
            crc32c: 0,
            sha256: Sha256::new(),
            sha1: Sha1::new(),
        }
    }
}
