// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::net::accept_any_cert::AcceptAnyCert;

pub(in crate::probe) fn build_tls_config() -> Result<std::sync::Arc<rustls::ClientConfig>, String> {
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = std::sync::Arc::new(AcceptAnyCert {
        provider: provider.clone(),
    });
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(std::sync::Arc::new(config))
}
