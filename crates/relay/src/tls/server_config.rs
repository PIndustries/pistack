// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The rustls config that presents the material. A bad PEM is unavailable;
//! nothing here calls the CA.

use super::existing_root::NO_CERT;
use super::material::TlsMaterial;
use pistack_types::{ApiError, ApiResult};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::sync::Arc;

/// rustls config that presents `material`. Parsing a bad PEM is unavailable.
/// Nothing here calls the CA.
pub fn server_config(material: &TlsMaterial) -> ApiResult<Arc<rustls::ServerConfig>> {
    let certs = CertificateDer::pem_slice_iter(material.cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    if certs.is_empty() {
        return Err(ApiError::unavailable(NO_CERT));
    }
    let key = PrivateKeyDer::from_pem_slice(material.key_pem.as_bytes())
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| ApiError::unavailable(NO_CERT))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}
