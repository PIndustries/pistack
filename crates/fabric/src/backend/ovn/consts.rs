// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::backend::ovn) const DB: &str = "OVN_Northbound";

pub(in crate::backend::ovn) const DROP_PRIORITY: i64 = 1001;

pub(in crate::backend::ovn) const RULE_PRIORITY: i64 = 1002;

/// Every port with port security joins this group. Its ACLs drop all IP
/// traffic both ways at a lower priority than group rules, and let DHCP
/// requests and metadata requests out whatever the port's groups say.
pub(in crate::backend::ovn) const DROP_GROUP: &str = "pg_pistack_drop";

/// `external_ids` key marking a switch port as the network's metadata port.
pub(in crate::backend::ovn) const METADATA_TAG: &str = "pistack:metadata";

/// `external_ids` key marking a router port as the router's gateway.
pub(in crate::backend::ovn) const GATEWAY_TAG: &str = "pistack:gateway";

/// The cluster term, as the cluster saves it in the store.
pub(in crate::backend::ovn) const TERM_KEY: &str = "cluster_term";
