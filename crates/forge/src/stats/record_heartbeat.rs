// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Host, Machine};
use gauge::catalog as names;
use pistack_rpc::proto::Heartbeat;
use pistack_store::Store;

use super::host_samples::host_samples;
use super::machine_samples::machine_samples;

/// Record what one heartbeat says about a host and its machines.
pub fn record_heartbeat(gauge: &gauge::Client, store: &Store, host: &Host, hb: &Heartbeat) {
    let node = host.spec.node_name.as_str();
    let on_host = [("host", node)];
    let mut out = vec![
        (
            gauge::series(names::HOST_VCPUS, &on_host),
            f64::from(host.spec.capacity.vcpus),
        ),
        (
            gauge::series(names::HOST_MEMORY_MIB, &on_host),
            host.spec.capacity.memory_mib as f64,
        ),
        (
            gauge::series(names::HOST_MACHINES, &on_host),
            hb.machines.len() as f64,
        ),
    ];
    if let Some(usage) = &hb.usage {
        host_samples(&mut out, node, &usage.host);
        let machines = store.collection::<Machine>();
        for m in &usage.machines {
            // A guest the store does not know is not ours to report.
            if let Ok(Some(machine)) = machines.try_get(&m.id) {
                machine_samples(&mut out, gauge, node, &machine, m);
            }
        }
    }
    gauge.record_all(out);
}
