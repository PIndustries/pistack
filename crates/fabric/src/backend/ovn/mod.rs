// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! OVN northbound backend. Networks, subnets, machine ports, security
//! groups, routers and floating IPs are written to OVN's northbound
//! database. The doc comment on each method is its contract.
//!
//! The PiStack store stays the record. OVN holds a copy the leader writes:
//! * Every row fabric creates carries `external_ids:pistack:id=<our id>`,
//!   `pistack:project` when the resource has one, and `pistack:term`, the
//!   cluster term of the leader that last wrote it. Rows without
//!   `pistack:id` are never touched.
//! * Each ensure reads the row first and writes only when it differs, so a
//!   repeat is a no-op. Inserts are guarded so a twin written since the
//!   read aborts the transaction instead of duplicating the row.
//! * Only the leader writes. Each transaction checks leadership first and
//!   is abandoned if leadership drops while it waits for the database. A
//!   transaction already sent may still commit; the new leader's full pass
//!   on promotion rewrites anything that differs from the store.
//! * Names: `Logical_Switch.name = ps-<net id>`, switch ports are named by
//!   the port id, a provider network's uplink is `provnet-<net id>`.
//!   A security group is `Port_Group.name = pg_<sg uuid without dashes>`
//!   (OVN names must be identifiers), and every port with port security
//!   also joins `pg_pistack_drop`. A router is `Logical_Router.name =
//!   pr-<rtr id>` and each of its ports is `lrp-<port id>`.
//! * Router gateway ports are scheduled on the chassis listed in
//!   `PISTACK_OVN_GATEWAY_CHASSIS`. With none listed, the chassis are the
//!   hosts whose agent reports a connected `ovn-controller` and a bridge
//!   mapping, by chassis name, at most [`MAX_GATEWAY_CHASSIS`]. The list
//!   is read again on the reconciler's timer, so a host that joins or
//!   drops out moves the gateway ports. With no such host the gateway
//!   port has no chassis, outbound NAT and floating IPs do not leave the
//!   cluster, and a warning says so.
//!
//! * Metadata: every switch has one more port, `pimeta-<net uuid>`, of
//!   `type=localport`, holding `169.254.169.254`. A localport exists on
//!   every chassis and its traffic never crosses a tunnel, so each host
//!   answers its own guests. DHCP hands guests an on-link route to that
//!   address. The host end is the agent's work; see `anvil`'s `ovnmeta`.
//!
//! On each host, OVS and `ovn-controller` realise the switches. The agent
//! plugs a machine's tap or a pod's link into the integration bridge with
//! `external_ids:iface-id=<port uuid>`; the port binding below tells it which
//! bridge and id to use.

mod acl;
mod acl_row_key;
mod address_entry;
mod attached;
mod bad_endpoint;
mod braces;
mod by_name;
mod chassis_from_env;
mod chassis_view;
mod consts;
mod dhcp_options;
mod drop_acls;
mod ext_has;
mod ext_value;
mod fmt_debug;
mod gateway_candidates;
mod gateway_candidates_tests;
mod group_acls;
mod group_name;
mod host_record;
mod host_status_view;
mod host_view;
mod lrp_name;
mod lsp_name;
mod metadata_port_mac;
mod metadata_port_name;
mod onto_router;
mod onto_switch;
mod ovn_backend;
mod ovnb;
mod ovs_net_backend;
mod pair_rows;
mod pick_dhcp;
mod port_network;
mod provnet_name;
mod pub_consts;
mod router_name;
mod router_port;
mod rule_acl;
mod run_tests;
mod same_acls;
mod server_mac;
mod switch_name;
mod tagged;
#[cfg(test)]
mod test_support;
mod unhook_nat;
mod uuid_strings;

pub use chassis_view::ChassisView;
pub use gateway_candidates::gateway_candidates;
pub use host_record::HostRecord;
pub use host_status_view::HostStatusView;
pub use host_view::HostView;
pub use metadata_port_mac::metadata_port_mac;
pub use metadata_port_name::metadata_port_name;
pub use ovn_backend::OvnBackend;
pub use pub_consts::{
    DEFAULT_NB, GATEWAY_CHASSIS_ENV, INTEGRATION_BRIDGE, MAX_GATEWAY_CHASSIS, METADATA_ADDRESS,
    NB_ENV,
};
