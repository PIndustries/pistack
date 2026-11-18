// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;

use super::sync_all::sync_all;

pub(crate) fn run_pass(client: &Client) {
    match sync_all(client) {
        Ok(0) => {}
        Ok(n) => tracing::info!(changed = n, "beacon auto-records synced"),
        Err(e) => tracing::warn!(error = %e, "beacon auto-records pass failed"),
    }
}
