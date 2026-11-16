// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Pod probes talk to the sandbox itself. Certificate names on that
/// address are not a trust decision, so the handshake checks the signature
/// and accepts any certificate.
#[derive(Debug)]
pub(crate) struct AcceptAnyCert {
    pub(super) provider: std::sync::Arc<rustls::crypto::CryptoProvider>,
}
