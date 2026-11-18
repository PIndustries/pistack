// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::net::IpAddr;

use fabric::{FloatingIp, Port};
use pistack_store::Store;
use pistack_types::{kind, Id};

use crate::types::Machine;

use super::guest::Guest;

/// The machine that holds `peer`. `scope` narrows it to a network and the
/// host the machine must be placed on.
pub(crate) fn lookup(store: &Store, peer: IpAddr, scope: Option<(&Id, &Id)>) -> Option<Guest> {
    let port = store
        .collection::<Port>()
        .list(None)
        .ok()?
        .into_iter()
        .find(|p| {
            p.spec.fixed_ips.contains(&peer)
                && scope.is_none_or(|(network, _)| &p.spec.network == network)
                && p.spec
                    .device
                    .as_ref()
                    .is_some_and(|d| d.kind == kind::MACHINE)
        })?;
    let device: Id = port.spec.device.clone()?;
    let machine = store.collection::<Machine>().get(&device).ok()?;
    if let Some((_, host)) = scope {
        if machine.status.host.as_ref() != Some(host) {
            return None;
        }
    }
    let public = store
        .collection::<FloatingIp>()
        .list(None)
        .ok()
        .into_iter()
        .flatten()
        .find(|f| f.spec.port.as_ref() == Some(&port.meta.id))
        .and_then(|f| f.spec.address)
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| peer.to_string());
    let key = machine
        .spec
        .ssh_keys
        .iter()
        .find(|k| !k.trim().is_empty())
        .cloned();
    Some(Guest {
        name: machine.meta.name.clone(),
        local: peer.to_string(),
        public,
        key,
        user_data: machine.spec.user_data.clone().unwrap_or_default(),
        project: machine.meta.project.clone(),
        secrets: machine.spec.secrets.clone(),
    })
}
