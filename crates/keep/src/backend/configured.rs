// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::sync::OnceLock;

use super::backend_err::backend_err;
use crate::error::Result;
use crate::KeyBackend;

/// The process's backend: parsed once from the environment. A bad setting
/// is returned on every call, so nothing silently runs on another backend.
pub fn configured() -> Result<&'static KeyBackend> {
    static CONFIGURED: OnceLock<std::result::Result<KeyBackend, String>> = OnceLock::new();
    CONFIGURED
        .get_or_init(|| KeyBackend::from_env().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| backend_err(e.clone()))
}
