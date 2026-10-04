// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The one writer of stacks. HTTP handlers call this. Apply inserts
//! networks, subnets and machines into the same store.

use crate::types::{validate_document, validate_spec, Stack, StackPhase, StackSpec, StackStatus};
use pistack_store::{Collection, Store, StoreError};
use pistack_types::{kind, ApiError, ApiResult, Id, Kind, Meta, Phase};
use serde::Deserialize;
use serde_json::Value;
use std::net::IpAddr;
use std::sync::Arc;

#[derive(Clone)]
pub struct Client {
    store: Store,
}

impl Client {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn stacks(&self) -> Collection<Stack> {
        self.store.collection()
    }

    pub fn create_stack(&self, mut meta: Meta, mut spec: StackSpec) -> ApiResult<Stack> {
        meta.id = Id::new(kind::STACK);
        if spec.name.trim().is_empty() {
            spec.name = meta.name.clone();
        }
        validate_spec(&mut spec)?;
        self.stacks()
            .create(Stack {
                meta,
                spec,
                status: StackStatus::accepted(),
            })
            .map_err(Into::into)
    }

    pub fn get_stack(&self, id: &Id) -> ApiResult<Stack> {
        self.stacks().get(id).map_err(Into::into)
    }

    pub fn resolve_stack(&self, project: Option<&Id>, name_or_id: &str) -> ApiResult<Stack> {
        self.stacks()
            .resolve(project, name_or_id)
            .map_err(Into::into)
    }

    pub fn list_stacks(&self, project: Option<&Id>) -> ApiResult<Vec<Stack>> {
        self.stacks().list(project).map_err(Into::into)
    }

    pub fn delete_stack(&self, id: &Id) -> ApiResult<Stack> {
        self.stacks().delete(id).map_err(Into::into)
    }

    /// Re-check `spec.document` and create the resources it names.
    ///
    /// A YAML stream, or one JSON object, of `network`, `subnet` and
    /// `machine` items is written into this store. A name that already
    /// exists in the stack's project is left as it is. Unknown kinds are
    /// reported and do not fail the apply. The phase stays `Accepted`.
    pub fn apply_stack(&self, id: &Id) -> ApiResult<Stack> {
        let cur = self.get_stack(id)?;
        validate_document(&cur.spec.document)?;
        let outcome = realize(&self.store, cur.meta.project.as_ref(), &cur.spec.document)?;
        let mut next = cur;
        next.status = StackStatus {
            phase: StackPhase::Accepted,
            message: outcome.message(),
            created: outcome.created,
        };
        let applied = self.stacks().update(next, None)?;
        tracing::debug!(
            id = %applied.meta.id,
            created = applied.status.created.len(),
            "stack applied"
        );
        Ok(applied)
    }
}

/// One resource object in a stack document.
struct DocItem {
    kind: String,
    name: String,
    spec: Value,
}

struct Applied {
    created: Vec<String>,
    present: Vec<String>,
    ignored: Vec<String>,
}

impl Applied {
    fn message(&self) -> String {
        let mut parts = Vec::new();
        if !self.created.is_empty() {
            parts.push(format!("created {}", self.created.join(", ")));
        }
        if !self.present.is_empty() {
            parts.push(format!("present {}", self.present.join(", ")));
        }
        if !self.ignored.is_empty() {
            parts.push(format!("ignored {}", self.ignored.join(", ")));
        }
        if parts.is_empty() {
            "accepted; no resources in the document".to_owned()
        } else {
            parts.join("; ")
        }
    }
}

#[derive(Deserialize)]
struct SubnetDoc {
    network: String,
    cidr: String,
    #[serde(default)]
    gateway: Option<IpAddr>,
    #[serde(default)]
    allocation_pools: Vec<fabric::AllocationPool>,
    #[serde(default)]
    dns_nameservers: Vec<IpAddr>,
    #[serde(default = "default_true")]
    enable_dhcp: bool,
    #[serde(default)]
    host_routes: Vec<fabric::HostRoute>,
}

fn default_true() -> bool {
    true
}

fn fabric_client(store: &Store) -> fabric::Client {
    fabric::Client::new(store.clone(), Arc::new(fabric::MockBackend::new()))
}

fn parse_document(document: &str) -> ApiResult<Vec<DocItem>> {
    let mut items = Vec::new();
    for doc in serde_yaml_ng::Deserializer::from_str(document) {
        let value = serde_json::Value::deserialize(doc)
            .map_err(|e| ApiError::bad_request(format!("document is not valid YAML: {e}")))?;
        if value.is_null() {
            continue;
        }
        if !value.is_object() {
            return Err(ApiError::bad_request(
                "each document must be a resource mapping",
            ));
        }
        let kind = value
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_owned();
        if kind.is_empty() {
            return Err(ApiError::bad_request("resource is missing kind"));
        }
        let name = value
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let spec = value.get("spec").cloned().unwrap_or(Value::Null);
        items.push(DocItem { kind, name, spec });
    }
    Ok(items)
}

fn spec_is_empty(spec: &Value) -> bool {
    spec.is_null() || spec.as_object().is_some_and(|m| m.is_empty())
}

fn require_name(kind_name: &str, name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request(format!(
            "{kind_name} is missing a name"
        )));
    }
    Ok(name.to_owned())
}

fn ignored_label(kind_name: &str, name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        format!("kind {kind_name}")
    } else {
        format!("kind {kind_name} ({name})")
    }
}

fn found<K: Kind>(store: &Store, project: Option<&Id>, name: &str) -> ApiResult<Option<K>> {
    match store.collection::<K>().get_by_name(project, name) {
        Ok(row) => Ok(Some(row)),
        Err(StoreError::NotFound(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn network_spec(name: &str, spec: &Value) -> ApiResult<fabric::NetworkSpec> {
    if spec_is_empty(spec) {
        return Ok(fabric::NetworkSpec::default());
    }
    serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("network {name}: {e}")))
}

fn find_network(
    store: &Store,
    project: Option<&Id>,
    name_or_id: &str,
    subnet: &str,
) -> ApiResult<fabric::Network> {
    let col = store.collection::<fabric::Network>();
    let net = if let Some(id) = Id::parse(kind::NETWORK, name_or_id) {
        match col.get(&id) {
            Ok(net) => net,
            Err(StoreError::NotFound(_)) => {
                return Err(ApiError::bad_request(format!(
                    "subnet {subnet}: network '{name_or_id}' was not found in this project"
                )))
            }
            Err(e) => return Err(e.into()),
        }
    } else {
        match col.get_by_name(project, name_or_id) {
            Ok(net) => net,
            Err(StoreError::NotFound(_)) => {
                return Err(ApiError::bad_request(format!(
                    "subnet {subnet}: network '{name_or_id}' was not found in this project"
                )))
            }
            Err(e) => return Err(e.into()),
        }
    };
    if net.meta.project.as_ref() != project {
        return Err(ApiError::bad_request(format!(
            "subnet {subnet}: network '{name_or_id}' is not in this project"
        )));
    }
    Ok(net)
}

fn subnet_spec(
    store: &Store,
    project: Option<&Id>,
    name: &str,
    spec: &Value,
) -> ApiResult<fabric::SubnetSpec> {
    if spec_is_empty(spec) {
        return Err(ApiError::bad_request(format!(
            "subnet {name}: spec.network and spec.cidr are required"
        )));
    }
    let doc: SubnetDoc = serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("subnet {name}: {e}")))?;
    let network_name = doc.network.trim();
    if network_name.is_empty() {
        return Err(ApiError::bad_request(format!(
            "subnet {name}: spec.network is required"
        )));
    }
    let cidr_raw = doc.cidr.trim();
    if cidr_raw.is_empty() {
        return Err(ApiError::bad_request(format!(
            "subnet {name}: spec.cidr is required"
        )));
    }
    let net = find_network(store, project, network_name, name)?;
    let cidr = cidr_raw.parse().map_err(|e| {
        ApiError::bad_request(format!("subnet {name}: invalid cidr '{cidr_raw}': {e}"))
    })?;
    Ok(fabric::SubnetSpec {
        network: net.meta.id.clone(),
        cidr,
        gateway: doc.gateway,
        allocation_pools: doc.allocation_pools,
        dns_nameservers: doc.dns_nameservers,
        enable_dhcp: doc.enable_dhcp,
        host_routes: doc.host_routes,
    })
}

fn machine_spec(name: &str, spec: &Value) -> ApiResult<forge::MachineSpec> {
    if spec_is_empty(spec) {
        return Err(ApiError::bad_request(format!(
            "machine {name}: size and image are required"
        )));
    }
    let spec: forge::MachineSpec = serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("machine {name}: {e}")))?;
    let size = spec.size.trim().to_owned();
    let image = spec.image.trim().to_owned();
    if size.is_empty() || image.is_empty() {
        return Err(ApiError::bad_request(format!(
            "machine {name}: size and image are required"
        )));
    }
    let mut spec = spec;
    spec.size = size;
    spec.image = image;
    Ok(spec)
}

fn realize(store: &Store, project: Option<&Id>, document: &str) -> ApiResult<Applied> {
    let items = parse_document(document)?;
    let fabric = fabric_client(store);
    let mut out = Applied {
        created: Vec::new(),
        present: Vec::new(),
        ignored: Vec::new(),
    };
    for item in items {
        match item.kind.to_ascii_lowercase().as_str() {
            "network" => {
                let name = require_name("network", &item.name)?;
                if let Some(existing) = found::<fabric::Network>(store, project, &name)? {
                    out.present.push(existing.meta.id.to_string());
                    continue;
                }
                let spec = network_spec(&name, &item.spec)?;
                let net = fabric
                    .create_network(Meta::new(kind::NETWORK, name, project.cloned()), spec)?;
                out.created.push(net.meta.id.to_string());
            }
            "subnet" => {
                let name = require_name("subnet", &item.name)?;
                if let Some(existing) = found::<fabric::Subnet>(store, project, &name)? {
                    out.present.push(existing.meta.id.to_string());
                    continue;
                }
                let spec = subnet_spec(store, project, &name, &item.spec)?;
                let sub =
                    fabric.create_subnet(Meta::new(kind::SUBNET, name, project.cloned()), spec)?;
                out.created.push(sub.meta.id.to_string());
            }
            "machine" => {
                let name = require_name("machine", &item.name)?;
                if let Some(existing) = found::<forge::Machine>(store, project, &name)? {
                    out.present.push(existing.meta.id.to_string());
                    continue;
                }
                let spec = machine_spec(&name, &item.spec)?;
                let mut machine = forge::new_machine(&name, project.cloned(), spec);
                machine.status.phase = Phase::Pending;
                let machine = store.collection::<forge::Machine>().create(machine)?;
                out.created.push(machine.meta.id.to_string());
            }
            _ => out.ignored.push(ignored_label(&item.kind, &item.name)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"
kind: network
name: app
spec: {}
---
kind: subnet
name: app-net
spec:
  network: app
  cidr: "10.20.0.0/24"
"#;

    #[test]
    fn apply_network_and_subnet_twice_does_not_duplicate() {
        let store = Store::in_memory().unwrap();
        let client = Client::new(store.clone());
        let project = Id::new(kind::PROJECT);
        let stack = client
            .create_stack(
                Meta::new(kind::STACK, "web", Some(project.clone())),
                StackSpec {
                    name: "web".into(),
                    document: DOC.into(),
                    parameters: Default::default(),
                },
            )
            .unwrap();

        let first = client.apply_stack(&stack.meta.id).unwrap();
        assert_eq!(first.status.phase, StackPhase::Accepted);
        let nets = store
            .collection::<fabric::Network>()
            .list(Some(&project))
            .unwrap();
        let subs = store
            .collection::<fabric::Subnet>()
            .list(Some(&project))
            .unwrap();
        assert_eq!(nets.len(), 1);
        assert_eq!(subs.len(), 1);
        let net = nets.iter().find(|n| n.meta.name == "app").unwrap();
        let sub = subs.iter().find(|s| s.meta.name == "app-net").unwrap();
        assert_eq!(sub.spec.network, net.meta.id);
        assert!(sub.spec.gateway.is_none());
        assert!(sub.spec.allocation_pools.is_empty());
        assert!(sub.spec.enable_dhcp);
        assert_eq!(sub.spec.cidr.to_string(), "10.20.0.0/24");
        assert_eq!(
            first.status.created,
            vec![net.meta.id.to_string(), sub.meta.id.to_string()]
        );
        assert!(first.status.message.contains(&net.meta.id.to_string()));
        assert!(first.status.message.contains(&sub.meta.id.to_string()));

        let second = client.apply_stack(&stack.meta.id).unwrap();
        assert_eq!(second.status.phase, StackPhase::Accepted);
        assert!(second.status.created.is_empty(), "{:?}", second.status);
        assert!(second.status.message.contains("present"));
        assert!(!second.status.message.contains("created"));
        let nets2 = store
            .collection::<fabric::Network>()
            .list(Some(&project))
            .unwrap();
        let subs2 = store
            .collection::<fabric::Subnet>()
            .list(Some(&project))
            .unwrap();
        assert_eq!(nets2.len(), 1);
        assert_eq!(subs2.len(), 1);
        assert_eq!(nets2[0].meta.id, net.meta.id);
        assert_eq!(subs2[0].meta.id, sub.meta.id);
        assert_eq!(nets2[0].meta.revision, net.meta.revision);
        assert_eq!(subs2[0].meta.revision, sub.meta.revision);
    }
}
