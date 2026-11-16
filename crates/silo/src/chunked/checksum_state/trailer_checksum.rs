// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Checksum algorithm named by `x-amz-trailer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrailerChecksum {
    Crc32,
    Crc32c,
    Sha256,
    Sha1,
}
