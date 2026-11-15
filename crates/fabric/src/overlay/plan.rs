// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::attach_router::attach_router;
use crate::overlay::canonical_provider::canonical_provider;
use crate::overlay::checks::claim::claim;
use crate::overlay::checks::overlay_mtu::overlay_mtu;
use crate::overlay::checks::unicast::unicast;
use crate::overlay::checks::v6_snat_agrees::v6_snat_agrees;
pub(in crate::overlay) use crate::overlay::consts::MAX_VNI;
use crate::overlay::consts::PLAN_VERSION;
use crate::overlay::device_hex::device_hex;
use crate::overlay::external_ips::external_ips;
use crate::overlay::plan_floating::plan_floating;
use crate::overlay::plan_ports::plan_ports;
use crate::overlay::types::overlay_plan::OverlayPlan;
use crate::overlay::types::plan_error::PlanError;
use crate::overlay::types::plan_host::PlanHost;
use crate::overlay::types::plan_network::PlanNetwork;
use crate::overlay::types::segment_plan::SegmentPlan;
use std::collections::BTreeSet;

/// Plan one host.
///
/// `peer_underlays` are other hosts on the underlay. A port whose underlay is
/// not this host is added too. This host is not a peer of itself. No process
/// is spawned.
pub fn plan(
    host: &PlanHost,
    networks: &[PlanNetwork],
    peer_underlays: &[String],
) -> Result<OverlayPlan, PlanError> {
    let local = unicast("underlay", &host.underlay)?;
    let local_s = local.to_string();
    let underlay_mtu = if host.underlay_mtu == 0 {
        1500
    } else {
        host.underlay_mtu
    };

    let mut peers = BTreeSet::new();
    for raw in peer_underlays {
        let ip = unicast("peer", raw)?;
        if ip != local {
            peers.insert(ip);
        }
    }
    let external_ips = external_ips(networks)?;

    let mut seen_net = BTreeSet::new();
    let mut seen_vni = BTreeSet::new();
    let mut seen_port = BTreeSet::new();
    let mut floating_ids = BTreeSet::new();
    let mut floating_addrs = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut segments = Vec::with_capacity(networks.len());
    let mut pending = Vec::new();

    for net in networks {
        if !seen_net.insert(net.id.clone()) {
            return Err(PlanError::Invalid(format!("duplicate network {}", net.id)));
        }
        // A provider segment is not VXLAN. VNI 0 is allowed, and its VNI is
        // not unique against other provider segments. Above the VXLAN max is
        // still refused.
        if net.provider.is_none() {
            if !(1..=MAX_VNI).contains(&net.vni) {
                return Err(PlanError::Vni(net.vni));
            }
            if !seen_vni.insert(net.vni) {
                return Err(PlanError::Invalid(format!("duplicate vni {}", net.vni)));
            }
        } else if net.vni > MAX_VNI {
            return Err(PlanError::Vni(net.vni));
        }
        let hex = device_hex(&net.id)?;
        let bridge = claim(&mut names, format!("po{hex}"), "po")?;
        let provider = match &net.provider {
            Some(spec) => Some(canonical_provider(spec, &hex, &mut names)?),
            None => None,
        };
        // No VXLAN name is claimed for a provider segment, so nothing installs one.
        let vxlan = if provider.is_some() {
            String::new()
        } else {
            claim(&mut names, format!("vx{hex}"), "vx")?
        };
        let mtu = overlay_mtu(net.mtu, underlay_mtu)?;
        let ports = plan_ports(net, local, &mut peers, &mut seen_port)?;
        let floating = plan_floating(
            net,
            &ports,
            &external_ips,
            &mut floating_ids,
            &mut floating_addrs,
            &mut names,
        )?;
        let at = segments.len();
        segments.push(SegmentPlan {
            id: net.id.clone(),
            vni: net.vni,
            mtu,
            bridge,
            vxlan,
            local: local_s.clone(),
            peers: Vec::new(),
            ports,
            routers: Vec::new(),
            metadata: net.routers.iter().any(|r| r.metadata),
            provider,
            floating,
        });
        for router in &net.routers {
            attach_router(&mut pending, &mut names, at, &net.id, &hex, router)?;
        }
    }

    let peer_list: Vec<String> = peers.iter().map(ToString::to_string).collect();
    for seg in &mut segments {
        if seg.provider.is_none() {
            seg.peers.clone_from(&peer_list);
        }
    }
    for mut router in pending {
        if router.ns.external_ip.is_none() {
            // No external address means no external veth, so no underlay to record.
            router.ns.external_dev.clear();
        }
        segments[router.at].routers.push(router.ns);
    }
    for seg in &segments {
        for ns in &seg.routers {
            v6_snat_agrees(ns)?;
        }
    }

    Ok(OverlayPlan {
        version: PLAN_VERSION,
        host: local_s,
        networks: segments,
    })
}
