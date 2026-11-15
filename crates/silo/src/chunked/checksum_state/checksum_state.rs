// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::trailer_checksum::TrailerChecksum;
use sha1::Sha1;
use sha2::Sha256;

pub(in crate::chunked) struct ChecksumState {
    pub(in crate::chunked) algo: TrailerChecksum,
    pub(in crate::chunked) crc32: crc32fast::Hasher,
    pub(in crate::chunked) crc32c: u32,
    pub(in crate::chunked) sha256: Sha256,
    pub(in crate::chunked) sha1: Sha1,
}
