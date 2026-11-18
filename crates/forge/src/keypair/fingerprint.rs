// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use base64::Engine as _;
use sha2::{Digest, Sha256};

use super::b64_nopad::B64_NOPAD;

/// The SHA256 fingerprint of a public key blob, as `ssh-keygen -l` prints it:
/// all 32 bytes of the digest in unpadded base64, which is 43 characters.
pub(crate) fn fingerprint(blob: &[u8]) -> String {
    let digest = Sha256::digest(blob);
    format!("SHA256:{}", B64_NOPAD.encode(digest))
}
