// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

/// An SSH public key a machine can be given. Only the public half is ever
/// stored; a pair generated here hands its private half to the caller once.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct KeypairSpec {
    /// The key as one OpenSSH line, what a guest is given verbatim.
    pub public_key: String,
    /// Wire name: `ssh-ed25519`, `ssh-rsa`, `ecdsa-sha2-nistp256`.
    pub key_type: String,
    /// `SHA256:...`, as `ssh-keygen -l` prints it.
    pub fingerprint: String,
}
