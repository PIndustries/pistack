// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;

use super::notified_key::NOTIFIED_KEY;

/// Write the acknowledged serial back, so the next pass leaves the zone alone
/// until its serial moves again.
pub(crate) fn record(client: &Client, id: &pistack_types::Id, serial: u32) {
    tracing::info!(serial, "beacon: secondaries notified");
    let want = serial.to_string();
    let _ = client.zones().modify(id, |z| {
        z.meta.annotations.insert(NOTIFIED_KEY.into(), want.clone());
        Ok(())
    });
}
