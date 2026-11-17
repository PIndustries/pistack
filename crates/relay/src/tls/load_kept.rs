// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Reading a listener's certificate and unsealing its private key out of
//! `keep`. Creates nothing.

use super::existing_root::existing_root;
use super::existing_root::NO_CERT;
use super::material::TlsMaterial;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};

/// Read `id` from `keep`. Does not create a root key, a CA, or a certificate.
pub fn load_kept(store: &Store, id: &Id) -> ApiResult<TlsMaterial> {
    let cert = store
        .collection::<keep::Certificate>()
        .get(id)
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    if cert.status.revoked || cert.status.cert_pem.trim().is_empty() {
        return Err(ApiError::unavailable(NO_CERT));
    }
    let key_id = cert
        .status
        .key_secret
        .as_ref()
        .ok_or_else(|| ApiError::unavailable(NO_CERT))?;
    let secret = store
        .collection::<keep::StoredSecret>()
        .get(key_id)
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    let sealed = secret
        .status
        .sealed
        .as_ref()
        .ok_or_else(|| ApiError::unavailable(NO_CERT))?;
    let root = existing_root(store)?;
    let key = root
        .open(sealed, key_id.to_string().as_bytes())
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    let key_pem = String::from_utf8(key).map_err(|_| ApiError::unavailable(NO_CERT))?;
    if key_pem.trim().is_empty() {
        return Err(ApiError::unavailable(NO_CERT));
    }
    Ok(TlsMaterial {
        cert_pem: cert.status.cert_pem,
        key_pem,
    })
}
