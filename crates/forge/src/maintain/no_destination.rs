// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::scheduler::Allocations;
use crate::types::Host;
use pistack_types::{Phase, Resources};

/// Why the drain found no destination. The scheduler saw only the hosts
/// that can take a live migration and answers in a tally of the whole fleet;
/// an operator needs host names, so this names one host per reason, in the
/// order the placement checks them.
pub(crate) fn no_destination(
    host: &Host,
    others: &[&Host],
    tried: &[&Host],
    size: Resources,
    alloc: &Allocations,
) -> String {
    if others.is_empty() {
        return format!(
            "host {} is the only host here, so there is nowhere to migrate to",
            host.meta.name
        );
    }
    if tried.is_empty() {
        return format!(
            "none of the {} other {} here can take a live migration",
            others.len(),
            if others.len() == 1 { "host" } else { "hosts" }
        );
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(h) = tried.iter().find(|h| h.status.phase != Phase::Ready) {
        parts.push(format!("{} is not ready", h.meta.name));
    }
    if let Some(h) = tried.iter().find(|h| h.spec.disabled) {
        parts.push(format!("{} is disabled", h.meta.name));
    }
    if let Some(h) = tried
        .iter()
        .find(|h| !size.fits_in(&h.status.free.saturating_sub(&alloc.used_on(&h.meta.id))))
    {
        parts.push(format!(
            "{} has no room for {} vcpus and {} MiB",
            h.meta.name, size.vcpus, size.memory_mib
        ));
    }
    if parts.is_empty() {
        return format!(
            "none of the {} hosts that could take it fits its networks, affinity or \
             host_selector",
            tried.len()
        );
    }
    parts.join(", ")
}
