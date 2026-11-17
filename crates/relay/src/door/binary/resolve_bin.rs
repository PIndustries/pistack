// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Resolving the publisher binary for a kind: an override env var wins over
//! `PATH`; a missing binary is unavailable, never a silent skip.

use super::super::kind::DoorKind;
use super::which::which;
use pistack_types::{ApiError, ApiResult};
use std::path::PathBuf;

pub(crate) fn resolve_bin(kind: DoorKind, fallback: &str) -> ApiResult<PathBuf> {
    let key = match kind {
        DoorKind::Cloudflared => "PISTACK_CLOUDFLARED",
        DoorKind::Tailscale => "PISTACK_TAILSCALE",
        DoorKind::Wireguard => "PISTACK_WG",
        DoorKind::Edge => {
            return Err(ApiError::internal("edge starts no process"));
        }
    };
    match std::env::var_os(key) {
        Some(v) if !v.is_empty() => {
            let path = PathBuf::from(&v);
            if path.is_file() {
                Ok(path)
            } else {
                Err(ApiError::unavailable(format!(
                    "{fallback} is not installed"
                )))
            }
        }
        Some(_) => Err(ApiError::unavailable(format!(
            "{fallback} is not installed"
        ))),
        None => which(fallback)
            .ok_or_else(|| ApiError::unavailable(format!("{fallback} is not installed"))),
    }
}
