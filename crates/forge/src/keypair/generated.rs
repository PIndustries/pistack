// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::public_key::PublicKey;

/// A generated `ed25519` pair: the public half to store, and the private key
/// once in OpenSSH's private key format.
pub struct Generated {
    pub public: PublicKey,
    pub private_openssh: String,
}
