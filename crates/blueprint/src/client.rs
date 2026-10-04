// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The one writer of stacks, kits, and workflows. HTTP handlers call this.
//! Apply inserts network, subnet, machine, router, pod, listener, and
//! record set rows into the same store.

use crate::types::{
    validate_document, validate_kit, validate_spec, validate_workflow, Kit, KitSpec, Stack,
    StackPhase, StackSpec, StackStatus, Workflow, WorkflowPhase, WorkflowSpec, WorkflowStatus,
    KIT_KIND, WORKFLOW_KIND,
};
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

    pub fn kits(&self) -> Collection<Kit> {
        self.store.collection()
    }

    pub fn workflows(&self) -> Collection<Workflow> {
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
    /// A YAML stream, or one JSON object, of `network`, `subnet`,
    /// `machine`, `router`, `pod`, `listener`, and DNS record set items is
    /// written into this store. A name that already exists in the stack's
    /// project is left as it is. Unknown kinds are reported and do not
    /// fail the apply. The phase stays `Accepted`.
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

    pub fn create_kit(&self, mut meta: Meta, mut spec: KitSpec) -> ApiResult<Kit> {
        meta.id = Id::new(KIT_KIND);
        if spec.name.trim().is_empty() {
            spec.name = meta.name.clone();
        }
        validate_kit(&mut spec)?;
        self.kits()
            .create(Kit {
                meta,
                spec,
                status: StackStatus::accepted(),
            })
            .map_err(Into::into)
    }

    pub fn get_kit(&self, id: &Id) -> ApiResult<Kit> {
        self.kits().get(id).map_err(Into::into)
    }

    pub fn resolve_kit(&self, project: Option<&Id>, name_or_id: &str) -> ApiResult<Kit> {
        self.kits().resolve(project, name_or_id).map_err(Into::into)
    }

    pub fn list_kits(&self, project: Option<&Id>) -> ApiResult<Vec<Kit>> {
        self.kits().list(project).map_err(Into::into)
    }

    pub fn delete_kit(&self, id: &Id) -> ApiResult<Kit> {
        self.kits().delete(id).map_err(Into::into)
    }

    /// Apply a stored kit document through [`realize`].
    pub fn apply_kit(&self, id: &Id) -> ApiResult<Kit> {
        let cur = self.get_kit(id)?;
        validate_document(&cur.spec.document)?;
        let outcome = realize(&self.store, cur.meta.project.as_ref(), &cur.spec.document)?;
        let mut next = cur;
        next.status = StackStatus {
            phase: StackPhase::Accepted,
            message: outcome.message(),
            created: outcome.created,
        };
        let applied = self.kits().update(next, None)?;
        tracing::debug!(
            id = %applied.meta.id,
            created = applied.status.created.len(),
            "kit applied"
        );
        Ok(applied)
    }

    pub fn create_workflow(&self, mut meta: Meta, mut spec: WorkflowSpec) -> ApiResult<Workflow> {
        meta.id = Id::new(WORKFLOW_KIND);
        if spec.name.trim().is_empty() {
            spec.name = meta.name.clone();
        }
        validate_workflow(&mut spec)?;
        self.workflows()
            .create(Workflow {
                meta,
                spec,
                status: WorkflowStatus::accepted(),
            })
            .map_err(Into::into)
    }

    pub fn get_workflow(&self, id: &Id) -> ApiResult<Workflow> {
        self.workflows().get(id).map_err(Into::into)
    }

    pub fn resolve_workflow(&self, project: Option<&Id>, name_or_id: &str) -> ApiResult<Workflow> {
        self.workflows()
            .resolve(project, name_or_id)
            .map_err(Into::into)
    }

    pub fn list_workflows(&self, project: Option<&Id>) -> ApiResult<Vec<Workflow>> {
        self.workflows().list(project).map_err(Into::into)
    }

    pub fn delete_workflow(&self, id: &Id) -> ApiResult<Workflow> {
        self.workflows().delete(id).map_err(Into::into)
    }

    /// Apply each named stack in order. The first error is stored on the
    /// workflow and returned. Stacks after that name are not applied.
    pub fn apply_workflow(&self, id: &Id) -> ApiResult<Workflow> {
        let cur = self.get_workflow(id)?;
        let project = cur.meta.project.clone();
        let names = cur.spec.stacks.clone();
        let mut applied = Vec::new();
        for name in &names {
            let stack = match self.resolve_stack(project.as_ref(), name) {
                Ok(stack) => stack,
                Err(e) => {
                    return self.stop_workflow(id, &applied, name, e);
                }
            };
            if let Err(e) = self.apply_stack(&stack.meta.id) {
                return self.stop_workflow(id, &applied, name, e);
            }
            applied.push(name.clone());
        }
        let message = if applied.is_empty() {
            "accepted; workflow has no stacks".to_owned()
        } else {
            format!("applied {}", applied.join(", "))
        };
        self.write_workflow(id, WorkflowPhase::Accepted, message, applied)
    }

    fn stop_workflow(
        &self,
        id: &Id,
        applied: &[String],
        name: &str,
        err: ApiError,
    ) -> ApiResult<Workflow> {
        let message = format!("stopped on stack {name}: {}", err.message);
        self.write_workflow(id, WorkflowPhase::Failed, message.clone(), applied.to_vec())?;
        Err(ApiError::new(err.code, message))
    }

    fn write_workflow(
        &self,
        id: &Id,
        phase: WorkflowPhase,
        message: String,
        applied: Vec<String>,
    ) -> ApiResult<Workflow> {
        let mut next = self.get_workflow(id)?;
        next.status = WorkflowStatus {
            phase,
            message,
            applied,
        };
        self.workflows().update(next, None).map_err(Into::into)
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

fn relay_client(store: &Store) -> relay::Client {
    relay::Client::new(
        store.clone(),
        relay::Providers::new(vec![Arc::new(relay::MockProvider::new())]),
    )
}

fn beacon_client(store: &Store) -> beacon::Client {
    beacon::Client::new(store.clone())
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

fn find_named<K: Kind>(
    store: &Store,
    project: Option<&Id>,
    name_or_id: &str,
    owner_kind: &str,
    owner: &str,
    target: &str,
) -> ApiResult<K> {
    let col = store.collection::<K>();
    let row = if let Some(id) = Id::parse(K::KIND, name_or_id) {
        match col.get(&id) {
            Ok(row) => row,
            Err(StoreError::NotFound(_)) => {
                return Err(ApiError::bad_request(format!(
                    "{owner_kind} {owner}: {target} '{name_or_id}' was not found in this project"
                )))
            }
            Err(e) => return Err(e.into()),
        }
    } else {
        match col.get_by_name(project, name_or_id) {
            Ok(row) => row,
            Err(StoreError::NotFound(_)) => {
                return Err(ApiError::bad_request(format!(
                    "{owner_kind} {owner}: {target} '{name_or_id}' was not found in this project"
                )))
            }
            Err(e) => return Err(e.into()),
        }
    };
    if row.meta().project.as_ref() != project {
        return Err(ApiError::bad_request(format!(
            "{owner_kind} {owner}: {target} '{name_or_id}' is not in this project"
        )));
    }
    Ok(row)
}

fn find_network(
    store: &Store,
    project: Option<&Id>,
    name_or_id: &str,
    subnet: &str,
) -> ApiResult<fabric::Network> {
    find_named(store, project, name_or_id, "subnet", subnet, "network")
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

#[derive(Deserialize)]
struct RouterDoc {
    #[serde(default)]
    external_network: Option<String>,
    #[serde(default)]
    interfaces: Vec<String>,
    #[serde(default)]
    routes: Vec<fabric::StaticRoute>,
}

fn router_spec(
    store: &Store,
    project: Option<&Id>,
    name: &str,
    spec: &Value,
) -> ApiResult<fabric::RouterSpec> {
    if spec_is_empty(spec) {
        return Ok(fabric::RouterSpec::default());
    }
    let doc: RouterDoc = serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("router {name}: {e}")))?;
    let external_network = match doc
        .external_network
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(network) => Some(
            find_named::<fabric::Network>(store, project, network, "router", name, "network")?
                .meta
                .id,
        ),
        None => None,
    };
    let mut interfaces = Vec::new();
    for iface in &doc.interfaces {
        let iface = iface.trim();
        if iface.is_empty() {
            return Err(ApiError::bad_request(format!(
                "router {name}: interface name is empty"
            )));
        }
        let subnet = find_named::<fabric::Subnet>(store, project, iface, "router", name, "subnet")?;
        interfaces.push(subnet.meta.id);
    }
    Ok(fabric::RouterSpec {
        external_network,
        interfaces,
        routes: doc.routes,
    })
}

fn pod_spec(name: &str, spec: &Value) -> ApiResult<loom::PodSpec> {
    if spec_is_empty(spec) {
        return Err(ApiError::bad_request(format!(
            "pod {name}: spec.containers is required"
        )));
    }
    let mut spec: loom::PodSpec = serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("pod {name}: {e}")))?;
    loom::http::Api::validate(&mut spec, None)
        .map_err(|e| ApiError::bad_request(format!("pod {name}: {}", e.message)))?;
    Ok(spec)
}

#[derive(Deserialize)]
struct ListenerDoc {
    #[serde(default)]
    balancer: String,
    #[serde(default)]
    vip_network: String,
    #[serde(default)]
    provider: Option<relay::Provider>,
    protocol: Value,
    port: u16,
    #[serde(default)]
    connection_limit: u32,
    #[serde(default = "default_true")]
    admin_up: bool,
}

fn listener_protocol(name: &str, value: &Value) -> ApiResult<relay::ListenerProtocol> {
    if let Some(protocol) = value.as_str() {
        return match protocol.to_ascii_lowercase().as_str() {
            "tcp" => Ok(relay::ListenerProtocol::Tcp),
            "udp" => Ok(relay::ListenerProtocol::Udp),
            "http" => Ok(relay::ListenerProtocol::Http),
            "https" => Ok(relay::ListenerProtocol::Https),
            other => Err(ApiError::bad_request(format!(
                "listener {name}: unknown protocol '{other}'"
            ))),
        };
    }
    serde_json::from_value(value.clone())
        .map_err(|e| ApiError::bad_request(format!("listener {name}: protocol: {e}")))
}

fn project_allows(mine: Option<&Id>, theirs: Option<&Id>) -> bool {
    match (mine, theirs) {
        (_, None) => true,
        (Some(a), Some(b)) => a == b,
        (None, Some(_)) => false,
    }
}

fn ensure_balancer(
    relay: &relay::Client,
    store: &Store,
    project: Option<&Id>,
    listener: &str,
    doc: &ListenerDoc,
) -> ApiResult<relay::Balancer> {
    let name = {
        let given = doc.balancer.trim();
        if given.is_empty() {
            listener.to_owned()
        } else {
            given.to_owned()
        }
    };
    if let Some(id) = Id::parse(kind::BALANCER, &name) {
        let balancer = store
            .collection::<relay::Balancer>()
            .get(&id)
            .map_err(ApiError::from)?;
        if !project_allows(project, balancer.meta.project.as_ref()) {
            return Err(ApiError::bad_request(format!(
                "listener {listener}: balancer '{name}' is not in this project"
            )));
        }
        return Ok(balancer);
    }
    if let Some(existing) = found::<relay::Balancer>(store, project, &name)? {
        return Ok(existing);
    }
    let vip = doc.vip_network.trim();
    if vip.is_empty() {
        return Err(ApiError::bad_request(format!(
            "listener {listener}: spec.vip_network is required to create balancer '{name}'"
        )));
    }
    let mut spec = relay::BalancerSpec {
        vip_network: vip.to_owned(),
        ..relay::BalancerSpec::default()
    };
    if let Some(provider) = doc.provider {
        spec.provider = provider;
    }
    relay
        .create_balancer(Meta::new(kind::BALANCER, name, project.cloned()), spec)
        .map_err(|e| ApiError::new(e.code, format!("listener {listener}: {}", e.message)))
}

#[derive(Deserialize)]
struct RecordDoc {
    #[serde(default)]
    zone: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    ttl: Option<u32>,
    #[serde(default)]
    records: Vec<String>,
}

fn parse_record_type(name: &str, raw: &str) -> ApiResult<beacon::RecordType> {
    match raw.trim().to_ascii_uppercase().as_str() {
        "A" => Ok(beacon::RecordType::A),
        "AAAA" => Ok(beacon::RecordType::Aaaa),
        "CNAME" => Ok(beacon::RecordType::Cname),
        "MX" => Ok(beacon::RecordType::Mx),
        "TXT" => Ok(beacon::RecordType::Txt),
        "SRV" => Ok(beacon::RecordType::Srv),
        "NS" => Ok(beacon::RecordType::Ns),
        "PTR" => Ok(beacon::RecordType::Ptr),
        "" => Err(ApiError::bad_request(format!(
            "dns {name}: spec.type is required"
        ))),
        other => Err(ApiError::bad_request(format!(
            "dns {name}: unknown type '{other}'"
        ))),
    }
}

fn ensure_zone(
    beacon: &beacon::Client,
    store: &Store,
    project: Option<&Id>,
    dns: &str,
    zone_ref: &str,
) -> ApiResult<beacon::Zone> {
    let zone_ref = zone_ref.trim();
    if zone_ref.is_empty() {
        return Err(ApiError::bad_request(format!(
            "dns {dns}: spec.zone is required"
        )));
    }
    if let Some(id) = Id::parse(kind::ZONE, zone_ref) {
        let zone = store
            .collection::<beacon::Zone>()
            .get(&id)
            .map_err(ApiError::from)?;
        if !project_allows(project, zone.meta.project.as_ref()) {
            return Err(ApiError::bad_request(format!(
                "dns {dns}: zone '{zone_ref}' is not in this project"
            )));
        }
        return Ok(zone);
    }
    if let Some(existing) = found::<beacon::Zone>(store, project, zone_ref)? {
        return Ok(existing);
    }
    let apex = beacon::normalize_name(zone_ref).map_err(|e| {
        ApiError::bad_request(format!("dns {dns}: zone '{zone_ref}': {}", e.message))
    })?;
    let meta_name = apex.trim_end_matches('.').to_owned();
    if meta_name != zone_ref {
        if let Some(existing) = found::<beacon::Zone>(store, project, &meta_name)? {
            return Ok(existing);
        }
    }
    if let Some(existing) = beacon.zone_by_apex(&apex)? {
        if project_allows(project, existing.meta.project.as_ref()) {
            return Ok(existing);
        }
        return Err(ApiError::bad_request(format!(
            "dns {dns}: zone '{zone_ref}' belongs to another project"
        )));
    }
    beacon
        .create_zone(
            Meta::new(kind::ZONE, meta_name, project.cloned()),
            beacon::ZoneSpec {
                name: apex,
                ..beacon::ZoneSpec::default()
            },
        )
        .map_err(|e| ApiError::new(e.code, format!("dns {dns}: {}", e.message)))
}

fn record_spec(
    beacon: &beacon::Client,
    store: &Store,
    project: Option<&Id>,
    name: &str,
    spec: &Value,
) -> ApiResult<beacon::RecordSetSpec> {
    if spec_is_empty(spec) {
        return Err(ApiError::bad_request(format!(
            "dns {name}: spec.zone, spec.type, and spec.records are required"
        )));
    }
    let doc: RecordDoc = serde_json::from_value(spec.clone())
        .map_err(|e| ApiError::bad_request(format!("dns {name}: {e}")))?;
    if doc.records.is_empty() {
        return Err(ApiError::bad_request(format!(
            "dns {name}: spec.records is required"
        )));
    }
    let kind_ = parse_record_type(name, &doc.kind)?;
    let zone = ensure_zone(beacon, store, project, name, &doc.zone)?;
    let owner = {
        let given = doc.name.trim();
        if given.is_empty() {
            name.to_owned()
        } else {
            given.to_owned()
        }
    };
    Ok(beacon::RecordSetSpec {
        zone: zone.meta.id,
        name: owner,
        kind: kind_,
        ttl: doc.ttl,
        records: doc.records,
    })
}

fn tag(out: &mut Applied, fresh: bool, id: String) {
    if fresh {
        out.created.push(id);
    } else {
        out.present.push(id);
    }
}

/// Create the resources named in `document`.
///
/// `network`, `subnet`, `machine`, `router`, `pod`, `listener`, and a DNS
/// record set (`dns`, `recordset`, `record_set`, or `record`) are written
/// into this store. A name that already exists in the project is left as
/// it is. A listener is stored through relay, which needs a balancer: the
/// named balancer is created when it is missing. A record set is stored
/// through beacon, which needs a zone: the named zone is created when it
/// is missing. Unknown kinds are reported and do not fail the apply.
fn realize(store: &Store, project: Option<&Id>, document: &str) -> ApiResult<Applied> {
    let items = parse_document(document)?;
    let fabric = fabric_client(store);
    let relay = relay_client(store);
    let beacon = beacon_client(store);
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
                    tag(&mut out, false, existing.meta.id.to_string());
                    continue;
                }
                let spec = network_spec(&name, &item.spec)?;
                let net = fabric
                    .create_network(Meta::new(kind::NETWORK, name, project.cloned()), spec)?;
                tag(&mut out, true, net.meta.id.to_string());
            }
            "subnet" => {
                let name = require_name("subnet", &item.name)?;
                if let Some(existing) = found::<fabric::Subnet>(store, project, &name)? {
                    tag(&mut out, false, existing.meta.id.to_string());
                    continue;
                }
                let spec = subnet_spec(store, project, &name, &item.spec)?;
                let sub =
                    fabric.create_subnet(Meta::new(kind::SUBNET, name, project.cloned()), spec)?;
                tag(&mut out, true, sub.meta.id.to_string());
            }
            "machine" => {
                let name = require_name("machine", &item.name)?;
                if let Some(existing) = found::<forge::Machine>(store, project, &name)? {
                    tag(&mut out, false, existing.meta.id.to_string());
                    continue;
                }
                let spec = machine_spec(&name, &item.spec)?;
                let mut machine = forge::new_machine(&name, project.cloned(), spec);
                machine.status.phase = Phase::Pending;
                let machine = store.collection::<forge::Machine>().create(machine)?;
                tag(&mut out, true, machine.meta.id.to_string());
            }
            "router" => {
                let name = require_name("router", &item.name)?;
                if let Some(existing) = found::<fabric::Router>(store, project, &name)? {
                    tag(&mut out, false, existing.meta.id.to_string());
                    continue;
                }
                let spec = router_spec(store, project, &name, &item.spec)?;
                let router =
                    fabric.create_router(Meta::new(kind::ROUTER, name, project.cloned()), spec)?;
                tag(&mut out, true, router.meta.id.to_string());
            }
            "pod" => {
                let name = require_name("pod", &item.name)?;
                if let Some(existing) = found::<loom::Pod>(store, project, &name)? {
                    tag(&mut out, false, existing.meta.id.to_string());
                    continue;
                }
                let spec = pod_spec(&name, &item.spec)?;
                let pod: loom::Pod = pistack_types::Resource::new(name, project.cloned(), spec);
                let pod = store.collection::<loom::Pod>().create(pod)?;
                tag(&mut out, true, pod.meta.id.to_string());
            }
            "listener" => {
                let name = require_name("listener", &item.name)?;
                if let Some(existing) = found::<relay::Listener>(store, project, &name)? {
                    tag(&mut out, false, existing.meta.id.to_string());
                    continue;
                }
                if spec_is_empty(&item.spec) {
                    return Err(ApiError::bad_request(format!(
                        "listener {name}: spec.protocol and spec.port are required"
                    )));
                }
                let doc: ListenerDoc = serde_json::from_value(item.spec.clone())
                    .map_err(|e| ApiError::bad_request(format!("listener {name}: {e}")))?;
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
                        },
                    )
                    .map_err(|e| {
                        ApiError::new(e.code, format!("listener {name}: {}", e.message))
                    })?;
                tag(&mut out, true, listener.meta.id.to_string());
            }
            "dns" | "record" | "recordset" | "record_set" => {
                let name = require_name("dns", &item.name)?;
                if let Some(existing) = found::<beacon::RecordSet>(store, project, &name)? {
                    tag(&mut out, false, existing.meta.id.to_string());
                    continue;
                }
                let spec = record_spec(&beacon, store, project, &name, &item.spec)?;
                let set = beacon
                    .create_recordset(
                        Meta::new(kind::RECORD, name.clone(), project.cloned()),
                        spec,
                    )
                    .map_err(|e| ApiError::new(e.code, format!("dns {name}: {}", e.message)))?;
                tag(&mut out, true, set.meta.id.to_string());
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

    /// One document: network, subnet, router, database machine, web pod,
    /// listener, and DNS name. An unknown kind stays ignored.
    const APP: &str = r#"
kind: network
name: app
spec: {}
---
kind: subnet
name: app-net
spec:
  network: app
  cidr: "10.20.0.0/24"
  gateway: "10.20.0.1"
---
kind: router
name: edge
spec:
  interfaces:
    - app-net
---
kind: machine
name: db
spec:
  size: s.1x1
  image: postgres
  networks:
    - app
---
kind: pod
name: web
spec:
  containers:
    - name: web
      image: web:1
  networks:
    - app
---
kind: listener
name: web-http
spec:
  balancer: web-http
  vip_network: app
  protocol: tcp
  port: 80
---
kind: dns
name: web.app.test
spec:
  zone: app.test
  name: web
  type: A
  records:
    - "10.20.0.10"
---
kind: share
name: data
spec: {}
"#;

    fn stack(client: &Client, project: &Id, name: &str, document: &str) -> Stack {
        client
            .create_stack(
                Meta::new(kind::STACK, name, Some(project.clone())),
                StackSpec {
                    name: name.into(),
                    document: document.into(),
                    parameters: Default::default(),
                },
            )
            .unwrap()
    }

    #[test]
    fn apply_app_document_twice_keeps_one_row_of_each() {
        let store = Store::in_memory().unwrap();
        let client = Client::new(store.clone());
        let project = Id::new(kind::PROJECT);
        let stack = stack(&client, &project, "app", APP);

        let first = client.apply_stack(&stack.meta.id).unwrap();
        assert_eq!(first.status.phase, StackPhase::Accepted);
        assert!(first.status.message.contains("ignored"));
        assert!(first.status.message.contains("share"));

        let nets = store
            .collection::<fabric::Network>()
            .list(Some(&project))
            .unwrap();
        let subs = store
            .collection::<fabric::Subnet>()
            .list(Some(&project))
            .unwrap();
        let routers = store
            .collection::<fabric::Router>()
            .list(Some(&project))
            .unwrap();
        let machines = store
            .collection::<forge::Machine>()
            .list(Some(&project))
            .unwrap();
        let pods = store
            .collection::<loom::Pod>()
            .list(Some(&project))
            .unwrap();
        let listeners = store
            .collection::<relay::Listener>()
            .list(Some(&project))
            .unwrap();
        let records = store
            .collection::<beacon::RecordSet>()
            .list(Some(&project))
            .unwrap();
        let balancers = store
            .collection::<relay::Balancer>()
            .list(Some(&project))
            .unwrap();
        let zones = store
            .collection::<beacon::Zone>()
            .list(Some(&project))
            .unwrap();
        let ports = store
            .collection::<fabric::Port>()
            .list(Some(&project))
            .unwrap();

        assert_eq!(nets.len(), 1, "network");
        assert_eq!(subs.len(), 1, "subnet");
        assert_eq!(routers.len(), 1, "router");
        assert_eq!(machines.len(), 1, "machine");
        assert_eq!(pods.len(), 1, "pod");
        assert_eq!(listeners.len(), 1, "listener");
        assert_eq!(records.len(), 1, "dns");
        assert_eq!(balancers.len(), 1, "balancer behind the listener");
        assert_eq!(zones.len(), 1, "zone behind the dns name");
        assert_eq!(ports.len(), 1, "router interface");

        let net = &nets[0];
        let sub = &subs[0];
        let router = &routers[0];
        let machine = &machines[0];
        let pod = &pods[0];
        let listener = &listeners[0];
        let record = &records[0];
        assert_eq!(net.meta.name, "app");
        assert_eq!(sub.meta.name, "app-net");
        assert_eq!(sub.spec.network, net.meta.id);
        assert_eq!(router.meta.name, "edge");
        assert_eq!(router.spec.interfaces, vec![sub.meta.id.clone()]);
        assert_eq!(machine.meta.name, "db");
        assert_eq!(machine.spec.image, "postgres");
        assert_eq!(machine.spec.networks, vec!["app".to_string()]);
        assert_eq!(machine.status.phase, Phase::Pending);
        assert_eq!(pod.meta.name, "web");
        assert_eq!(pod.spec.containers[0].image, "web:1");
        assert_eq!(pod.status.phase, loom::PodPhase::Pending);
        assert_eq!(listener.meta.name, "web-http");
        assert_eq!(listener.spec.port, 80);
        assert_eq!(listener.spec.protocol, relay::ListenerProtocol::Tcp);
        assert_eq!(listener.spec.balancer, balancers[0].meta.id);
        assert_eq!(listener.status.phase, Phase::Pending);
        assert_eq!(record.meta.name, "web.app.test");
        assert_eq!(record.spec.name, "web.app.test.");
        assert_eq!(record.spec.kind, beacon::RecordType::A);
        assert_eq!(record.spec.records, vec!["10.20.0.10".to_string()]);
        assert_eq!(record.spec.zone, zones[0].meta.id);
        assert_eq!(record.status.phase, Phase::Pending);
        assert_eq!(
            first.status.created,
            vec![
                net.meta.id.to_string(),
                sub.meta.id.to_string(),
                router.meta.id.to_string(),
                machine.meta.id.to_string(),
                pod.meta.id.to_string(),
                listener.meta.id.to_string(),
                record.meta.id.to_string(),
            ]
        );

        let second = client.apply_stack(&stack.meta.id).unwrap();
        assert_eq!(second.status.phase, StackPhase::Accepted);
        assert!(second.status.created.is_empty(), "{:?}", second.status);
        assert!(second.status.message.contains("present"));
        assert!(second.status.message.contains("ignored"));
        assert!(!second.status.message.contains("created"));

        assert_eq!(
            store
                .collection::<fabric::Network>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .collection::<fabric::Subnet>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .collection::<fabric::Router>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .collection::<forge::Machine>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .collection::<loom::Pod>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .collection::<relay::Listener>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .collection::<beacon::RecordSet>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .collection::<relay::Balancer>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .collection::<beacon::Zone>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .collection::<fabric::Port>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );

        let net2 = store
            .collection::<fabric::Network>()
            .get(&net.meta.id)
            .unwrap();
        let sub2 = store
            .collection::<fabric::Subnet>()
            .get(&sub.meta.id)
            .unwrap();
        let router2 = store
            .collection::<fabric::Router>()
            .get(&router.meta.id)
            .unwrap();
        let machine2 = store
            .collection::<forge::Machine>()
            .get(&machine.meta.id)
            .unwrap();
        let pod2 = store.collection::<loom::Pod>().get(&pod.meta.id).unwrap();
        let listener2 = store
            .collection::<relay::Listener>()
            .get(&listener.meta.id)
            .unwrap();
        let record2 = store
            .collection::<beacon::RecordSet>()
            .get(&record.meta.id)
            .unwrap();
        assert_eq!(net2.meta.revision, net.meta.revision);
        assert_eq!(sub2.meta.revision, sub.meta.revision);
        assert_eq!(router2.meta.revision, router.meta.revision);
        assert_eq!(machine2.meta.revision, machine.meta.revision);
        assert_eq!(pod2.meta.revision, pod.meta.revision);
        assert_eq!(listener2.meta.revision, listener.meta.revision);
        assert_eq!(record2.meta.revision, record.meta.revision);
    }

    #[test]
    fn kit_apply_uses_realize_and_does_not_duplicate() {
        let store = Store::in_memory().unwrap();
        let client = Client::new(store.clone());
        let project = Id::new(kind::PROJECT);
        let document =
            "kind: network\nname: kit-net\nspec: {}\n---\nkind: router\nname: kit-edge\nspec: {}\n";
        let kit = client
            .create_kit(
                Meta::new(KIT_KIND, "base", Some(project.clone())),
                KitSpec {
                    name: "base".into(),
                    document: document.into(),
                    parameters: Default::default(),
                },
            )
            .unwrap();
        assert_eq!(kit.status.message, crate::types::ACCEPTED_MESSAGE);

        let first = client.apply_kit(&kit.meta.id).unwrap();
        assert_eq!(first.status.created.len(), 2, "{:?}", first.status);
        assert_eq!(
            store
                .collection::<fabric::Network>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .collection::<fabric::Router>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );

        let second = client.apply_kit(&kit.meta.id).unwrap();
        assert!(second.status.created.is_empty(), "{:?}", second.status);
        assert!(second.status.message.contains("present"));
        assert_eq!(
            store
                .collection::<fabric::Network>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .collection::<fabric::Router>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn workflow_applies_stacks_in_order_and_stops_on_error() {
        let store = Store::in_memory().unwrap();
        let client = Client::new(store.clone());
        let project = Id::new(kind::PROJECT);
        stack(
            &client,
            &project,
            "first",
            "kind: network\nname: app\nspec: {}\n",
        );
        stack(
            &client,
            &project,
            "bad",
            "kind: machine\nname: broken\nspec: {}\n",
        );
        stack(
            &client,
            &project,
            "later",
            "kind: network\nname: later-net\nspec: {}\n",
        );
        let wf = client
            .create_workflow(
                Meta::new(WORKFLOW_KIND, "rollout", Some(project.clone())),
                WorkflowSpec {
                    name: "rollout".into(),
                    stacks: vec!["first".into(), "bad".into(), "later".into()],
                },
            )
            .unwrap();
        assert_eq!(wf.status.message, crate::types::WORKFLOW_ACCEPTED_MESSAGE);

        let err = client.apply_workflow(&wf.meta.id).unwrap_err();
        assert!(err.message.contains("stopped on stack bad"), "{err}");
        assert!(err.message.contains("size and image"), "{err}");

        let stored = client.get_workflow(&wf.meta.id).unwrap();
        assert_eq!(stored.status.phase, WorkflowPhase::Failed);
        assert_eq!(stored.status.applied, vec!["first".to_string()]);

        let nets = store
            .collection::<fabric::Network>()
            .list(Some(&project))
            .unwrap();
        assert_eq!(nets.len(), 1);
        assert_eq!(nets[0].meta.name, "app");
        assert!(store
            .collection::<forge::Machine>()
            .list(Some(&project))
            .unwrap()
            .is_empty());

        let again = client
            .create_workflow(
                Meta::new(WORKFLOW_KIND, "again", Some(project.clone())),
                WorkflowSpec {
                    name: "again".into(),
                    stacks: vec!["first".into()],
                },
            )
            .unwrap();
        let done = client.apply_workflow(&again.meta.id).unwrap();
        assert_eq!(done.status.phase, WorkflowPhase::Accepted);
        assert_eq!(done.status.applied, vec!["first".to_string()]);
        assert!(done.status.message.contains("applied first"));
        assert_eq!(
            store
                .collection::<fabric::Network>()
                .list(Some(&project))
                .unwrap()
                .len(),
            1
        );
    }
}
