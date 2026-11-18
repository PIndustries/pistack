// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::SocketAddr;

use super::default_listen::DEFAULT_LISTEN;
use super::parse_listen::parse_listen;

pub(crate) fn listen_from_env() -> Option<SocketAddr> {
    let text = std::env::var("PISTACK_METADATA_LISTEN").unwrap_or_else(|_| DEFAULT_LISTEN.into());
    match parse_listen(&text) {
        Ok(addr) => Some(addr),
        Err(msg) => {
            tracing::error!(listen = %text, "{msg}");
            None
        }
    }
}
