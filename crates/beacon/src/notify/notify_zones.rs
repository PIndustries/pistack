// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use parking_lot::Mutex;
use pistack_types::module::CancellationToken;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

use super::next_id::next_id;
use super::notify_bytes::notify_bytes;
use super::peers_for::peers_for;
use super::record::record;
use super::send_notify::send_notify;

pub(crate) async fn notify_zones(
    client: &Client,
    todo: Vec<(pistack_types::Id, String, u32, crate::types::Zone)>,
) {
    let mut joins = Vec::new();
    let acked: Arc<Mutex<HashSet<(String, SocketAddr)>>> = Arc::new(Mutex::new(HashSet::new()));
    for (id, apex, serial, zone) in todo.iter() {
        let peers = match peers_for(client, zone) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(zone = %apex, error = %e, "beacon: notify peers");
                Vec::new()
            }
        };
        for to in peers {
            let bytes = match notify_bytes(next_id(), apex, *serial) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(zone = %apex, error = %e, "beacon: notify encode");
                    continue;
                }
            };
            let (acked, apex, id) = (Arc::clone(&acked), apex.clone(), id.clone());
            joins.push(tokio::spawn(async move {
                let ok = send_notify(&bytes, to, &acked, &apex, &CancellationToken::new()).await;
                if ok {
                    acked.lock().insert((apex, to));
                }
                (id, ok)
            }));
        }
    }
    let mut done: HashMap<pistack_types::Id, bool> = HashMap::new();
    for j in joins {
        match j.await {
            Ok((id, ok)) => *done.entry(id).or_insert(true) &= ok,
            Err(e) => tracing::debug!(error = %e, "beacon: notify task"),
        }
    }
    for (id, apex, serial, _) in todo {
        match done.get(&id) {
            // Acknowledged by everyone the zone names. A zone with no secondaries
            // has nobody to wait for and is marked the same way.
            Some(true) | None => record(client, &id, serial),
            // Unanswered: leave the mark where it is. The reconcile timer
            // brings the next pass, which will try again.
            _ => tracing::debug!(zone = %apex, serial, "beacon: notify unacknowledged"),
        }
    }
}
