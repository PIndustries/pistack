// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Stopping the publisher this process started for a listener.

use super::stop_process::stop_process;

/// Stop the publisher this process started for `id`. Edge has no process.
/// WireGuard is not deleted here, and `wg-fleet` is never named.
pub fn stop(id: &str) {
    super::super::edge::edge_published().lock().remove(id);
    stop_process(id);
}
