// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::http::StatusCode;
use pistack_store::Store;

use super::guest::Guest;

/// Payload of a secret named on this machine, in the machine's project.
/// Any other name is 404. A keep store that will not open is 503, and the
/// body is not a payload.
pub(crate) fn named_secret(store: &Store, guest: &Guest, name: &str) -> (StatusCode, Vec<u8>) {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\0')
        || name == "."
        || name == ".."
        || !guest.secrets.iter().any(|s| s == name)
    {
        return (StatusCode::NOT_FOUND, Vec::new());
    }
    let client = match keep::Client::open(store) {
        Ok(c) => c,
        Err(_) => {
            tracing::error!("forge: metadata secret store is unavailable");
            return (StatusCode::SERVICE_UNAVAILABLE, b"unavailable".to_vec());
        }
    };
    let listed = match client.list_secrets(guest.project.as_ref()) {
        Ok(v) => v,
        Err(_) => return (StatusCode::NOT_FOUND, Vec::new()),
    };
    let Some(secret) = listed
        .into_iter()
        .find(|s| s.meta.name == name && s.meta.project == guest.project)
    else {
        return (StatusCode::NOT_FOUND, Vec::new());
    };
    match client.get_secret_payload(&secret.meta.id) {
        Ok(bytes) => (StatusCode::OK, bytes),
        Err(_) => (StatusCode::NOT_FOUND, Vec::new()),
    }
}
