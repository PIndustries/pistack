// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! OVN northbound backend. **Stub**: every operation returns
//! `not_implemented`, but the doc comment on each one is the contract for
//! the real thing: which NB tables and columns it touches and how PiStack
//! ids appear in `external_ids` so a later reconciler can find and garbage
//! collect rows.
//!
//! Conventions once implemented:
//! * Every row fabric creates carries `external_ids:pistack:id=<our id>`
//!   and `external_ids:pistack:project=<project id>`. Rows without them
//!   are never touched.
//! * All ops are OVSDB transactions with `wait`/`mutate` guards so they
//!   are idempotent: ensure = "insert if absent, then set columns".
//! * Names: `Logical_Switch.name = ps-<net id>`,
//!   `Logical_Router.name = pr-<rtr id>`, LSP/LRP names are the port id,
//!   `Port_Group.name = pg_<sg uuid with '-' replaced by '_'>` (OVN names
//!   must be valid identifiers), `Address_Set` likewise with `as_` prefix.

use super::{FipView, NetBackend, RouterView};
use crate::types::{Network, Port, PortBinding, SecurityGroup, Subnet};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};

/// Connection details for the NB database. Unused until the client lands.
#[derive(Clone, Debug)]
pub struct OvnBackend {
    /// e.g. `tcp:127.0.0.1:6641` or `ssl:...`.
    pub nb_endpoint: String,
}

impl OvnBackend {
    pub fn new(nb_endpoint: impl Into<String>) -> Self {
        Self {
            nb_endpoint: nb_endpoint.into(),
        }
    }

    fn todo(&self, op: &str) -> ApiError {
        ApiError::not_implemented(format!(
            "ovn backend: {op} not implemented (nb={})",
            self.nb_endpoint
        ))
        .with_detail("op", op)
    }
}

#[async_trait]
impl NetBackend for OvnBackend {
    fn name(&self) -> &'static str {
        "ovn"
    }

    /// `Logical_Switch`:
    /// * `name = ps-<net id>`
    /// * `other_config:mtu = <spec.mtu>`
    /// * Overlay: nothing else; OVN picks the Geneve tunnel key. We mirror
    ///   it into `status.segment_id` from `other_config` once SB is read.
    /// * Provider: a `Logical_Switch_Port` named `provnet-<net id>` with
    ///   `type=localnet`, `options:network_name=<physnet>`,
    ///   `addresses=["unknown"]`, and `tag=<vlan>` when set.
    /// * `external` networks additionally get
    ///   `external_ids:pistack:external=true` so routers can find them.
    /// * `other_config:vlan-passthru` stays unset (no Q-in-Q).
    async fn ensure_network(&self, _net: &Network) -> ApiResult<()> {
        Err(self.todo("ensure_network"))
    }

    /// Delete the `Logical_Switch` (`ls-del`); OVN cascades its ports.
    /// Also removes any `DHCP_Options` rows whose
    /// `external_ids:pistack:network` matches.
    async fn delete_network(&self, _id: &Id) -> ApiResult<()> {
        Err(self.todo("delete_network"))
    }

    /// `DHCP_Options` (one row per subnet with `enable_dhcp`):
    /// * `cidr = <spec.cidr>`
    /// * `options:server_id = <gateway or first pool address>`,
    ///   `options:server_mac = <a generated OUI mac>`,
    ///   `options:lease_time = 43200`, `options:router = <gateway>`,
    ///   `options:dns_server = {<dns_nameservers>}`,
    ///   `options:mtu = <network mtu>`,
    ///   `options:classless_static_route = {<host_routes>, 0.0.0.0/0,<gw>}`
    /// * IPv6: `options:server_id` is the gateway MAC, `options:dns_server`
    ///   as above; no router option.
    /// * `external_ids:pistack:subnet = <subnet id>`,
    ///   `external_ids:pistack:network = <net id>`.
    ///
    /// The `Logical_Switch` gets `other_config:subnet = <cidr>` (IPv4) or
    /// `other_config:ipv6_prefix` so OVN can do dynamic addressing if we
    /// ever let it; today fabric allocates every address itself.
    async fn ensure_subnet(&self, _net: &Network, _subnet: &Subnet) -> ApiResult<()> {
        Err(self.todo("ensure_subnet"))
    }

    /// Delete the subnet's `DHCP_Options` row and clear the switch's
    /// `other_config:subnet`.
    async fn delete_subnet(&self, _id: &Id) -> ApiResult<()> {
        Err(self.todo("delete_subnet"))
    }

    /// `Logical_Switch_Port` on `ps-<net id>`:
    /// * `name = <port id>`, `type = ""` (VIF) for machine ports;
    ///   `type = router` + `options:router-port = <lrp name>` for router
    ///   ports (see [`Self::ensure_router`]).
    /// * `addresses = ["<mac> <ip1> <ip2>..."]`.
    /// * `port_security = ["<mac> <ip1>..."]` when `spec.port_security`,
    ///   else `[]`.
    /// * `dhcpv4_options` / `dhcpv6_options` = the subnet's `DHCP_Options`
    ///   row when the subnet has DHCP enabled.
    /// * `external_ids:pistack:id`, `pistack:device = <device id>`,
    ///   `pistack:project`.
    /// * Security groups: add the LSP to each group's `Port_Group.ports`;
    ///   when `port_security` is off the port joins no group and receives
    ///   no ACLs (OVN default allow).
    ///
    /// Binding: `anvil` sets `external_ids:iface-id = <port id>` on the OVS
    /// interface on the host; OVN SB `Port_Binding.chassis` then tells us
    /// `status.host`. The returned `PortBinding` is
    /// `{vif_type: "ovs", vif_details: {bridge: "br-int", iface_id: <port id>}}`.
    async fn ensure_port(
        &self,
        _net: &Network,
        _port: &Port,
        _groups: &[SecurityGroup],
    ) -> ApiResult<PortBinding> {
        Err(self.todo("ensure_port"))
    }

    /// `lsp-del <port id>`; remove it from every `Port_Group.ports`.
    async fn delete_port(&self, _id: &Id) -> ApiResult<()> {
        Err(self.todo("delete_port"))
    }

    /// `Logical_Router`:
    /// * `name = pr-<rtr id>`, `options:always_learn_from_arp_request=false`,
    ///   `options:dynamic_neigh_routers=true`.
    /// * Per interface subnet: `Logical_Router_Port`
    ///   `name = lrp-<port id>`, `mac = <port mac>`,
    ///   `networks = ["<gateway>/<prefix>"]`, linked from the
    ///   `Logical_Switch_Port` `type=router`,
    ///   `options:router-port=lrp-<port id>`, `addresses=["router"]`.
    /// * Gateway: an LRP on the external switch the same way, plus
    ///   `Gateway_Chassis` rows (one per gateway-capable chassis, priority
    ///   descending) so the port is scheduled; a default
    ///   `Logical_Router_Static_Route` `ip_prefix=0.0.0.0/0`,
    ///   `nexthop=<external subnet gateway>`; and a `NAT` row
    ///   `type=snat`, `logical_ip=<each interface cidr>`,
    ///   `external_ip=<gateway port fixed ip>` so tenant traffic can leave.
    /// * `spec.routes` become `Logical_Router_Static_Route` rows
    ///   (`ip_prefix`, `nexthop`).
    /// * Everything carries `external_ids:pistack:id = <rtr id>`; the
    ///   reconciler diffs existing LRP/NAT/route rows against the view and
    ///   removes the extras.
    async fn ensure_router(&self, _view: &RouterView) -> ApiResult<()> {
        Err(self.todo("ensure_router"))
    }

    /// `lr-del pr-<rtr id>` (cascades LRPs, NAT, routes); delete the
    /// matching `type=router` LSPs on each switch.
    async fn delete_router(&self, _id: &Id) -> ApiResult<()> {
        Err(self.todo("delete_router"))
    }

    /// One `Port_Group` per security group:
    /// * `name = pg_<sg uuid>`, `ports = {member LSPs}`,
    ///   `external_ids:pistack:id = <sg id>`.
    /// * OVN derives `Address_Set`s `pg_<uuid>_ip4` / `_ip6` from the
    ///   group's ports automatically; those are what `Remote::Group` rules
    ///   match on.
    /// * Per rule an `ACL` in `Port_Group.acls`:
    ///   `direction = to-lport` (ingress) or `from-lport` (egress),
    ///   `priority = 1002`, `action = allow-related`,
    ///   `match = "outport == @pg_<uuid> && ip4 && ip4.src == <cidr> && tcp && tcp.dst >= min && tcp.dst <= max"`
    ///   (ingress; `inport`/`ip4.dst` for egress; `ip6` for IPv6;
    ///   `Remote::Group(g)` → `ip4.src == $pg_<g uuid>_ip4`;
    ///   `Remote::Any` omits the address term; `protocol=None|Any`
    ///   omits the L4 term; `icmp` → `icmp4`/`icmp6`).
    /// * Two implicit drop ACLs per group at `priority = 1001`:
    ///   `to-lport outport == @pg && ip` and `from-lport inport == @pg && ip`,
    ///   `action = drop`, so a port with groups is default-deny both ways
    ///   except for what its rules allow.
    /// * A cloud-wide `Port_Group` `pg_drop_all` (priority 1000 drop) and
    ///   allow ACLs for DHCP (`udp.src == 68 && udp.dst == 67`) and
    ///   IPv6 ND are installed once by the reconciler on first use.
    /// * ACL `external_ids:pistack:rule = <rule uuid>` lets the reconciler
    ///   diff rules without recreating the whole group.
    async fn apply_security_group(&self, _sg: &SecurityGroup, _members: &[Port]) -> ApiResult<()> {
        Err(self.todo("apply_security_group"))
    }

    /// `pg-del pg_<sg uuid>`; OVN cascades its ACLs.
    async fn delete_security_group(&self, _id: &Id) -> ApiResult<()> {
        Err(self.todo("delete_security_group"))
    }

    /// `NAT` row on the router that has an interface on the target port's
    /// subnet (`view.router`):
    /// * `type = dnat_and_snat`, `external_ip = <fip address>`,
    ///   `logical_ip = <target fixed ip>`, `logical_port = <port id>`,
    ///   `external_mac = <gateway port mac>` (distributed FIP, so the
    ///   chassis hosting the VM answers ARP directly),
    ///   `options:stateless = false`,
    ///   `external_ids:pistack:id = <fip id>`.
    /// * Disassociate (`target: None`): remove the NAT row matching
    ///   `external_ids:pistack:id`.
    /// * The address itself is reserved in the external subnet by fabric's
    ///   IPAM; OVN needs nothing for an unassociated FIP.
    async fn associate_fip(&self, _view: &FipView) -> ApiResult<()> {
        Err(self.todo("associate_fip"))
    }

    /// Remove any `NAT` row with `external_ids:pistack:id = <fip id>`.
    async fn delete_fip(&self, _id: &Id) -> ApiResult<()> {
        Err(self.todo("delete_fip"))
    }
}
