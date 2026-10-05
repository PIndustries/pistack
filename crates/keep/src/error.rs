// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `keep`'s library error. HTTP handlers convert it to `ApiError`; the
//! binary and other modules see it through `keep::Client`.

use pistack_store::StoreError;
use pistack_types::ApiError;

#[derive(Debug, thiserror::Error)]
pub enum KeepError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// Encryption or decryption failed: wrong key, tampered blob, bad encoding.
    #[error("crypto: {0}")]
    Crypto(String),
    /// Certificate generation, parsing or verification failed.
    #[error("pki: {0}")]
    Pki(String),
    #[error("tls: {0}")]
    Tls(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid: {0}")]
    Invalid(String),
    /// A join token or certificate that is unknown, used up, revoked or expired.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// The request clashes with state `keep` manages itself.
    #[error("conflict: {0}")]
    Conflict(String),
    /// The root-key backend is misconfigured, unreachable or refused.
    #[error("key backend: {0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, KeepError>;

impl From<KeepError> for ApiError {
    fn from(e: KeepError) -> Self {
        match e {
            KeepError::Store(s) => s.into(),
            KeepError::Crypto(m) => ApiError::internal(format!("crypto: {m}")),
            KeepError::Pki(m) => ApiError::unprocessable(format!("pki: {m}")),
            KeepError::Tls(m) => ApiError::internal(format!("tls: {m}")),
            KeepError::NotFound(m) => ApiError::not_found(m),
            KeepError::Invalid(m) => ApiError::bad_request(m),
            KeepError::Unauthorized(m) => ApiError::unauthorized(m),
            KeepError::Conflict(m) => ApiError::conflict(m),
            KeepError::Backend(m) => ApiError::unavailable(format!("key backend: {m}")),
        }
    }
}

impl From<rcgen::Error> for KeepError {
    fn from(e: rcgen::Error) -> Self {
        KeepError::Pki(e.to_string())
    }
}

impl From<rustls::Error> for KeepError {
    fn from(e: rustls::Error) -> Self {
        KeepError::Tls(e.to_string())
    }
}
