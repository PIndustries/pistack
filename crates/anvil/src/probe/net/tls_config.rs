// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::net::build_tls_config::build_tls_config;
use std::sync::OnceLock;

pub(in crate::probe) fn tls_config() -> Result<std::sync::Arc<rustls::ClientConfig>, String> {
    static CELL: OnceLock<Result<std::sync::Arc<rustls::ClientConfig>, String>> = OnceLock::new();
    CELL.get_or_init(build_tls_config).clone()
}
