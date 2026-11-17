// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{kind, ApiError, ApiResult, Id, Meta, Phase};
use std::collections::BTreeSet;

use super::applied::Applied;
use super::beacon_client::beacon_client;
use super::delete_orphans::delete_orphans;
use super::ensure_balancer::ensure_balancer;
use super::fabric_client::fabric_client;
use super::found::found;
use super::ignored_label::ignored_label;
use super::listener_doc::listener_doc;
use super::listener_protocol::listener_protocol;
use super::machine_spec::machine_spec;
use super::network_spec::network_spec;
use super::parse_document::parse_document;
use super::pod_spec::pod_spec;
use super::record_spec::record_spec;
use super::relay_client::relay_client;
use super::require_name::require_name;
use super::router_spec::router_spec;
use super::subnet_spec::subnet_spec;
use super::tag::tag;
use super::update_listener::update_listener;
use super::update_machine::update_machine;
use super::update_network::update_network;
use super::update_pod::update_pod;
use super::update_record::update_record;
use super::update_router::update_router;
use super::update_subnet::update_subnet;

/// Realize the resources named in `document`.
///
/// `network`, `subnet`, `machine`, `router`, `pod`, `listener`, and a DNS
/// record set (`dns`, `recordset`, `record_set`, or `record`) are written
/// into this store. A missing name is created. An existing name whose id
/// is in `managed` is updated to the document's spec through its owning
/// module's update rules; any other existing name is left as it is. A
/// listener is stored through relay, which needs a balancer: the named
/// balancer is created when it is missing. A record set is stored through
/// beacon, which needs a zone: the named zone is created when it is
/// missing. Unknown kinds are reported and do not fail the apply. Managed
/// ids the document no longer names are deleted in dependency order
/// (record sets and listeners before pods and machines, then routers,
/// subnets, and networks). Machines and pods are marked for deletion;
/// their controllers finish the work. Resources the stack never created
/// stay untouched.
///
/// `out` holds what was written even when an item fails part way.
pub(crate) fn realize(
    store: &Store,
    project: Option<&Id>,
    document: &str,
    managed: &[String],
    out: &mut Applied,
) -> ApiResult<()> {
    let items = parse_document(document)?;
    let fabric = fabric_client(store);
    let relay = relay_client(store);
    let beacon = beacon_client(store);
    let owned = |id: &str| managed.iter().any(|m| m == id);
    for item in items {
        match item.kind.to_ascii_lowercase().as_str() {
            "network" => {
                let name = require_name("network", &item.name)?;
                if let Some(existing) = found::<fabric::Network>(store, project, &name)? {
                    let id = existing.meta.id.to_string();
                    let changed =
                        owned(&id) && update_network(&fabric, &existing, &name, &item.spec)?;
                    out.keep(id, changed);
                    continue;
                }
                let spec = network_spec(&name, &item.spec)?;
                let net = fabric
                    .create_network(Meta::new(kind::NETWORK, name, project.cloned()), spec)?;
                tag(out, true, net.meta.id.to_string());
            }
            "subnet" => {
                let name = require_name("subnet", &item.name)?;
                if let Some(existing) = found::<fabric::Subnet>(store, project, &name)? {
                    let id = existing.meta.id.to_string();
                    let changed = owned(&id)
                        && update_subnet(&fabric, store, project, &existing, &name, &item.spec)?;
                    out.keep(id, changed);
                    continue;
                }
                let spec = subnet_spec(store, project, &name, &item.spec)?;
                let sub =
                    fabric.create_subnet(Meta::new(kind::SUBNET, name, project.cloned()), spec)?;
                tag(out, true, sub.meta.id.to_string());
            }
            "machine" => {
                let name = require_name("machine", &item.name)?;
                if let Some(existing) = found::<forge::Machine>(store, project, &name)? {
                    let id = existing.meta.id.to_string();
                    let changed =
                        owned(&id) && update_machine(store, &existing, &name, &item.spec)?;
                    out.keep(id, changed);
                    continue;
                }
                let spec = machine_spec(&name, &item.spec)?;
                let mut machine = forge::new_machine(&name, project.cloned(), spec);
                machine.status.phase = Phase::Pending;
                let machine = store.collection::<forge::Machine>().create(machine)?;
                tag(out, true, machine.meta.id.to_string());
            }
            "router" => {
                let name = require_name("router", &item.name)?;
                if let Some(existing) = found::<fabric::Router>(store, project, &name)? {
                    let id = existing.meta.id.to_string();
                    let changed = owned(&id)
                        && update_router(&fabric, store, project, &existing, &name, &item.spec)?;
                    out.keep(id, changed);
                    continue;
                }
                let spec = router_spec(store, project, &name, &item.spec)?;
                let router =
                    fabric.create_router(Meta::new(kind::ROUTER, name, project.cloned()), spec)?;
                tag(out, true, router.meta.id.to_string());
            }
            "pod" => {
                let name = require_name("pod", &item.name)?;
                if let Some(existing) = found::<loom::Pod>(store, project, &name)? {
                    let id = existing.meta.id.to_string();
                    let changed = owned(&id) && update_pod(store, &existing, &name, &item.spec)?;
                    out.keep(id, changed);
                    continue;
                }
                let spec = pod_spec(&name, &item.spec)?;
                let pod: loom::Pod = pistack_types::Resource::new(name, project.cloned(), spec);
                let pod = store.collection::<loom::Pod>().create(pod)?;
                tag(out, true, pod.meta.id.to_string());
            }
            "listener" => {
                let name = require_name("listener", &item.name)?;
                if let Some(existing) = found::<relay::Listener>(store, project, &name)? {
                    let id = existing.meta.id.to_string();
                    let changed =
                        owned(&id) && update_listener(&relay, store, &existing, &name, &item.spec)?;
                    out.keep(id, changed);
                    continue;
                }
                let doc = listener_doc(&name, &item.spec)?;
                let protocol = listener_protocol(&name, &doc.protocol)?;
                let balancer = ensure_balancer(&relay, store, project, &name, &doc)?;
                let listener = relay
                    .create_listener(
                        Meta::new(relay::kinds::LISTENER, name.clone(), project.cloned()),
                        relay::ListenerSpec {
                            balancer: balancer.meta.id,
                            protocol,
                            port: doc.port,
                            default_pool: None,
                            connection_limit: doc.connection_limit,
                            admin_up: doc.admin_up,
                            door: None,
                            routes: Vec::new(),
                        },
                    )
                    .map_err(|e| {
                        ApiError::new(e.code, format!("listener {name}: {}", e.message))
                    })?;
                tag(out, true, listener.meta.id.to_string());
            }
            "dns" | "record" | "recordset" | "record_set" => {
                let name = require_name("dns", &item.name)?;
                if let Some(existing) = found::<beacon::RecordSet>(store, project, &name)? {
                    let id = existing.meta.id.to_string();
                    let changed = owned(&id)
                        && update_record(&beacon, store, project, &existing, &name, &item.spec)?;
                    out.keep(id, changed);
                    continue;
                }
                let spec = record_spec(&beacon, store, project, &name, &item.spec)?;
                let set = beacon
                    .create_recordset(
                        Meta::new(kind::RECORD, name.clone(), project.cloned()),
                        spec,
                    )
                    .map_err(|e| ApiError::new(e.code, format!("dns {name}: {}", e.message)))?;
                tag(out, true, set.meta.id.to_string());
            }
            _ => out.ignored.push(ignored_label(&item.kind, &item.name)),
        }
    }
    let touched: BTreeSet<&String> = out
        .created
        .iter()
        .chain(&out.updated)
        .chain(&out.present)
        .collect();
    let orphans: Vec<String> = managed
        .iter()
        .filter(|id| !touched.contains(id))
        .cloned()
        .collect();
    delete_orphans(store, &fabric, &relay, &beacon, &orphans, out)?;
    Ok(())
}
