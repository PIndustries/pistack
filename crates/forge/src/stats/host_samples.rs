// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use gauge::catalog as names;
use pistack_rpc::proto::HostUsage;

pub(crate) fn host_samples(out: &mut Vec<(String, f64)>, node: &str, usage: &HostUsage) {
    let on_host = [("host", node)];
    let mut put = |name: &str, value: f64| out.push((gauge::series(name, &on_host), value));
    for (name, milli) in [names::HOST_LOAD1, names::HOST_LOAD5, names::HOST_LOAD15]
        .into_iter()
        .zip(usage.load_milli)
    {
        put(name, f64::from(milli) / 1000.0);
    }
    if let Some(total) = usage.memory_total_bytes {
        put(names::HOST_MEMORY_TOTAL, total as f64);
        if let Some(available) = usage.memory_available_bytes {
            put(
                names::HOST_MEMORY_USED,
                total.saturating_sub(available) as f64,
            );
        }
    }
    if let Some(total) = usage.data_disk_total_bytes {
        put(names::HOST_DISK_TOTAL, total as f64);
    }
    if let Some(free) = usage.data_disk_free_bytes {
        put(names::HOST_DISK_FREE, free as f64);
    }
}
