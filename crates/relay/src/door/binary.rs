// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Finding the publisher's binary. An override environment variable wins
//! over `PATH`; a missing binary is unavailable, never a silent skip.

use super::kind::DoorKind;
use pistack_types::ApiError;
use std::path::PathBuf;

pub(super) fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

pub(super) fn resolve_bin(kind: DoorKind, fallback: &str) -> Result<PathBuf, ApiError> {
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
