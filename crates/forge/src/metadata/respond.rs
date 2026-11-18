// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::http::StatusCode;
use pistack_store::Store;

use super::guest::Guest;
use super::named_secret::named_secret;
use super::secret_names::secret_names;

pub(crate) fn respond(store: &Store, guest: Guest, path: &str) -> (StatusCode, Vec<u8>) {
    let path = path.trim_end_matches('/');
    if let Some(name) = path.strip_prefix("/latest/meta-data/secrets/") {
        return named_secret(store, &guest, name);
    }
    let body = match path {
        "/latest/meta-data" => {
            Some(b"hostname\nlocal-ipv4\npublic-ipv4\npublic-keys\nsecrets".to_vec())
        }
        // A value is the bare value and a listing has no last newline, as
        // the EC2 service answers. `$(curl ...)` hid the difference; a
        // program that reads the body whole did not.
        "/latest/meta-data/hostname" => Some(guest.name.into_bytes()),
        "/latest/meta-data/local-ipv4" => Some(guest.local.into_bytes()),
        "/latest/meta-data/public-ipv4" => Some(guest.public.into_bytes()),
        "/latest/meta-data/public-keys" => guest.key.as_ref().map(|_| b"0".to_vec()),
        "/latest/meta-data/public-keys/0/openssh-key" => {
            guest.key.map(|k| k.trim_end().to_owned().into_bytes())
        }
        "/latest/meta-data/secrets" => Some(secret_names(&guest.secrets)),
        "/latest/user-data" => Some(guest.user_data.into_bytes()),
        _ => None,
    };
    match body {
        Some(body) => (StatusCode::OK, body),
        None => (StatusCode::NOT_FOUND, Vec::new()),
    }
}
