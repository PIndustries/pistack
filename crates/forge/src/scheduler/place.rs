// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Host, MachineSpec, ZONE_LABEL};
use pistack_types::{ApiError, ApiResult, Id, Phase, Resources};

use super::allocations::Allocations;
use super::rejections::Rejections;
use super::score::Score;
use super::strategy::Strategy;

/// Pick a host for `spec` needing `size`.
pub fn place(
    spec: &MachineSpec,
    size: &Resources,
    hosts: &[Host],
    allocations: &Allocations,
    strategy: Strategy,
) -> ApiResult<Id> {
    let mut rej = Rejections::default();
    let accepted = spec.kind.accepted_drivers();

    let mut candidates: Vec<(Score, &Host)> = Vec::new();
    for h in hosts {
        if h.status.phase != Phase::Ready {
            rej.not_ready += 1;
            continue;
        }
        if h.spec.disabled {
            rej.disabled += 1;
            continue;
        }
        if !h
            .spec
            .drivers
            .iter()
            .any(|d| accepted.contains(&d.as_str()))
        {
            rej.kind += 1;
            continue;
        }
        let in_zone = spec
            .zone
            .as_ref()
            .is_none_or(|z| h.meta.labels.get(ZONE_LABEL) == Some(z));
        if !in_zone
            || !spec
                .host_selector
                .iter()
                .all(|(k, v)| h.meta.labels.get(k) == Some(v))
        {
            rej.selector += 1;
            continue;
        }
        let free = h
            .status
            .free
            .saturating_sub(&allocations.used_on(&h.meta.id));
        if !size.fits_in(&free) {
            rej.resources += 1;
            continue;
        }
        let after = free.saturating_sub(size);
        let on_host = allocations.machines_on(&h.meta.id);
        let affinity = spec.affinity.iter().filter(|n| on_host.contains(n)).count() as i64
            - spec
                .anti_affinity
                .iter()
                .filter(|n| on_host.contains(n))
                .count() as i64;
        let memory = match strategy {
            Strategy::Spread => after.memory_mib as i128,
            Strategy::Pack => -(after.memory_mib as i128),
        };
        candidates.push((
            Score {
                affinity,
                memory,
                vcpus: after.vcpus,
            },
            h,
        ));
    }

    if candidates.is_empty() {
        return Err(ApiError::unavailable(format!(
            "no host can take a {:?} machine of size {}: {} hosts, {} not ready, {} disabled, {} lack a driver, {} fail the selector, {} lack resources",
            spec.kind,
            spec.size,
            hosts.len(),
            rej.not_ready,
            rej.disabled,
            rej.kind,
            rej.selector,
            rej.resources
        ))
        .with_detail("hosts", hosts.len() as u64)
        .with_detail("not_ready", rej.not_ready as u64)
        .with_detail("disabled", rej.disabled as u64)
        .with_detail("no_driver", rej.kind as u64)
        .with_detail("selector", rej.selector as u64)
        .with_detail("resources", rej.resources as u64));
    }

    // Highest score wins; ties fall to the lowest id so the result is
    // deterministic regardless of input order.
    candidates.sort_by(|(sa, ha), (sb, hb)| sb.cmp(sa).then_with(|| ha.meta.id.cmp(&hb.meta.id)));
    Ok(candidates[0].1.meta.id.clone())
}
