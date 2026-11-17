// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The one writer of stacks, kits, and workflows. HTTP handlers call this.
//! Apply inserts network, subnet, machine, router, pod, listener, and
//! record set rows into the same store, updates the rows an earlier apply
//! of the same stack or kit inserted, and deletes managed rows the
//! document no longer names (dependents first).

mod about;
mod already_gone;
mod applied;
mod beacon_client;
mod block_on_async;
mod client_type;
mod default_true;
mod delete_orphans;
mod delete_owned;
mod delete_rank;
mod doc_item;
mod doc_item_type;
mod ensure_balancer;
mod ensure_zone;
mod fabric_client;
mod find_named;
mod find_network;
mod finish;
mod found;
mod ignored_label;
mod listener_doc;
mod listener_doc_type;
mod listener_protocol;
mod machine_spec;
mod managed_ids;
mod network_spec;
mod parse_all;
mod parse_document;
mod parse_record_type;
mod pod_spec;
mod project_allows;
mod realize;
mod record_doc;
mod record_revision;
mod record_spec;
mod relay_client;
mod request_machine_delete;
mod require_name;
mod router_doc;
mod router_spec;
mod spec_is_empty;
mod subnet_doc;
mod subnet_spec;
mod tag;
#[cfg(test)]
mod tests;
mod update_listener;
mod update_machine;
mod update_network;
mod update_pod;
mod update_record;
mod update_router;
mod update_subnet;
mod zone_matches;

pub use client_type::Client;
