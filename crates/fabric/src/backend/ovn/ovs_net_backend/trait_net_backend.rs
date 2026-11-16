// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

// `NetBackend` is an `#[async_trait]` trait, so Rust requires its whole
// implementation in one block. Each method's body lives in its own file as
// `OvnBackend::<method>_body`; this block is what stitches them together.

use super::super::super::FipView;
use super::super::super::NetBackend;
use super::super::super::RouterView;
use super::super::ovn_backend::OvnBackend;
use crate::types::{Network, Port, PortBinding, SecurityGroup, Subnet};
use async_trait::async_trait;
use pistack_store::Store;
use pistack_types::{ApiResult, Id, Leadership};

#[async_trait]
impl NetBackend for OvnBackend {
    fn name(&self) -> &'static str {
        self.name_body()
    }
    /// Schedule every gateway port again from the current chassis list.
    /// Nothing is written for a port whose chassis are already right.
    async fn refresh(&self) -> ApiResult<()> {
        self.refresh_body().await
    }
    fn attach(&self, leadership: &Leadership, store: &Store) {
        self.attach_body(leadership, store)
    }
    /// `Logical_Switch`:
    /// * `name = ps-<net id>`
    /// * `other_config:mtu = <spec.mtu>`
    /// * Overlay: nothing else; OVN picks the Geneve tunnel key.
    /// * Provider: a `Logical_Switch_Port` named `provnet-<net id>` with
    ///   `type=localnet`, `options:network_name=<physnet>`,
    ///   `addresses=["unknown"]`, and `tag=<vlan>` when set.
    /// * `external` networks additionally get
    ///   `external_ids:pistack:external=true` so routers can find them.
    /// * `other_config:vlan-passthru` stays unset (no Q-in-Q).
    /// * Every network: a `Logical_Switch_Port` named `pimeta-<net uuid>`
    ///   with `type=localport`,
    ///   `addresses=["<stable 06: mac> 169.254.169.254"]` and
    ///   `external_ids:pistack:metadata=true`.
    async fn ensure_network(&self, net: &Network) -> ApiResult<()> {
        self.ensure_network_body(net).await
    }
    /// Delete the `Logical_Switch`; OVN drops its ports with it. Also
    /// removes the network's `DHCP_Options` rows. A second delete succeeds.
    async fn delete_network(&self, id: &Id) -> ApiResult<()> {
        self.delete_network_body(id).await
    }
    /// `DHCP_Options` (one row per subnet with `enable_dhcp`):
    /// * `cidr = <spec.cidr>`
    /// * `options:server_id = <gateway or first host address>`,
    ///   `options:server_mac = <a stable locally administered mac>`,
    ///   `options:lease_time = 43200`, `options:router = <gateway>`,
    ///   `options:dns_server = {<dns_nameservers>}`,
    ///   `options:mtu = <network mtu>`,
    ///   `options:classless_static_route =
    ///   {169.254.169.254/32,0.0.0.0, <host_routes>, 0.0.0.0/0,<gw>}`:
    ///   metadata on-link, then the subnet's own routes, then the default
    ///   route when there is a gateway.
    /// * IPv6: `options:server_id` is the server mac, `options:dns_server`
    ///   as above; no router option.
    /// * `external_ids:pistack:subnet = <subnet id>`,
    ///   `external_ids:pistack:network = <net id>`.
    ///
    /// A subnet without DHCP has no row; turning DHCP off deletes it.
    /// Fabric allocates every address itself, so the switch does not get
    /// `other_config:subnet`.
    async fn ensure_subnet(&self, net: &Network, subnet: &Subnet) -> ApiResult<()> {
        self.ensure_subnet_body(net, subnet).await
    }
    /// Delete the subnet's `DHCP_Options` row. Ports referring to it lose
    /// the reference with it.
    async fn delete_subnet(&self, id: &Id) -> ApiResult<()> {
        self.delete_subnet_body(id).await
    }
    /// `Logical_Switch_Port` on `ps-<net id>`:
    /// * `name = <port uuid>`, `type = ""` (VIF) for machine ports. Router
    ///   ports are written with their router (see [`Self::ensure_router`]);
    ///   here they only get a binding.
    /// * `addresses = ["<mac> <ip1> <ip2>..."]`, or `["unknown"]` before a
    ///   mac is assigned. An allowed address pair joins it only as an
    ///   address it can name: a single address bare, `"<pair mac or the port
    ///   mac> <address>"`; a wider prefix never, since the column takes plain
    ///   addresses, and then only its MAC, `"<pair mac>"`, and only when that
    ///   mac is not the port's own — that is all the switch needs to know
    ///   where the mac lives.
    /// * `port_security` holds every entry `addresses` would carry plus each
    ///   pair with its mask, `"<pair mac or the port mac> <pair prefix>"`,
    ///   when `spec.port_security`, so a pair passes anti-spoofing, else `[]`.
    /// * `dhcpv4_options` / `dhcpv6_options` = the network's `DHCP_Options`
    ///   row whose range holds one of the port's addresses.
    /// * `external_ids:pistack:id`, `pistack:device = <device id>`,
    ///   `pistack:project`, `pistack:term`.
    /// * Security groups: with `port_security` the port joins
    ///   `pg_pistack_drop` and each of its groups' `Port_Group`, and leaves
    ///   any other PiStack group; without it the port is in no group and
    ///   gets no ACLs (OVN default allow).
    ///
    /// Binding: the agent sets `external_ids:iface-id = <port uuid>` on the
    /// OVS interface on the host. The returned `PortBinding` is
    /// `{vif_type: "ovs", vif_details: {bridge: "br-int", iface_id: <port uuid>}}`.
    async fn ensure_port(
        &self,
        net: &Network,
        port: &Port,
        groups: &[SecurityGroup],
    ) -> ApiResult<PortBinding> {
        self.ensure_port_body(net, port, groups).await
    }
    /// Take the port off its switch and delete it. A second delete
    /// succeeds. `Port_Group.ports` refers to switch ports weakly, so the
    /// port leaves its groups with it.
    async fn delete_port(&self, id: &Id) -> ApiResult<()> {
        self.delete_port_body(id).await
    }
    /// `Logical_Router`:
    /// * `name = pr-<rtr id>`, `options:always_learn_from_arp_request=false`,
    ///   `options:dynamic_neigh_routers=true`.
    /// * Per interface subnet: `Logical_Router_Port`
    ///   `name = lrp-<port id>`, `mac = <port mac>`,
    ///   `networks = ["<gateway>/<prefix>"]`, linked from the
    ///   `Logical_Switch_Port` `type=router`,
    ///   `options:router-port=lrp-<port id>`, `addresses=["router"]`.
    /// * Gateway: an LRP on the external switch the same way, with
    ///   `networks = ["<gateway port ip>/<external subnet prefix>"]`, plus
    ///   `Gateway_Chassis` rows `name = lrp-<port id>_<chassis>` (one per
    ///   chassis in `PISTACK_OVN_GATEWAY_CHASSIS`, priority descending) so
    ///   the port is scheduled; a default `Logical_Router_Static_Route`
    ///   `ip_prefix=0.0.0.0/0` (`::/0` for IPv6),
    ///   `nexthop=<external subnet gateway>`; and a `NAT` row per IPv4
    ///   interface subnet, `type=snat`, `logical_ip=<interface cidr>`,
    ///   `external_ip=<gateway port fixed ip>`, so tenant traffic can leave.
    /// * `spec.routes` become `Logical_Router_Static_Route` rows
    ///   (`ip_prefix`, `nexthop`).
    /// * LRPs, routes, router NAT rows and gateway chassis carry
    ///   `external_ids:pistack:id = <rtr id>` (LRPs also `pistack:port`).
    ///   Each ensure compares them with the view: LRPs no longer wanted go
    ///   with their switch peers, and the route and router NAT sets are
    ///   replaced only when they differ. Floating IP NAT rows on the router
    ///   carry their own id and are left alone.
    async fn ensure_router(&self, view: &RouterView) -> ApiResult<()> {
        self.ensure_router_body(view).await
    }
    /// Delete `pr-<rtr id>`; its LRPs, NAT rows, routes and gateway chassis
    /// go with it (none of those tables is a root table). Then take the
    /// router's `type=router` ports off their switches. A second delete
    /// succeeds.
    async fn delete_router(&self, id: &Id) -> ApiResult<()> {
        self.delete_router_body(id).await
    }
    /// One `Port_Group` per security group:
    /// * `name = pg_<sg uuid without dashes>`, `ports = {member switch
    ///   ports with port security}`, `external_ids:pistack:id = <sg id>`.
    /// * OVN derives address sets `pg_<uuid>_ip4` / `_ip6` from the
    ///   group's ports; `Remote::Group` rules match on those. A group a
    ///   rule names is created empty if it does not exist yet.
    /// * Per rule an `ACL` in `Port_Group.acls`: `direction = to-lport`
    ///   (ingress) or `from-lport` (egress), `priority = 1002`,
    ///   `action = allow-related`, and a match such as
    ///   `outport == @pg_<uuid> && ip4 && ip4.src == 10.0.0.0/8 && tcp && tcp.dst == 22`
    ///   (`inport` and `ip4.dst` for egress, `ip6` for IPv6, `icmp4` /
    ///   `icmp6` for ICMP; no address term for `Remote::Any`, no L4 term
    ///   for any protocol). A rule whose remote address is of the other
    ///   family matches nothing and gets no ACL.
    /// * Default deny lives in one shared group, `pg_pistack_drop`, that
    ///   every port with port security joins: drop all IP both ways at
    ///   `priority = 1001`, allow DHCP and DHCPv6 requests out at 1002.
    /// * ACLs carry `external_ids:pistack:group = <group name>` and
    ///   `pistack:rule = <rule id>`. When the rules change, the group's
    ///   ACL set is replaced in one transaction; unchanged rules write
    ///   nothing.
    async fn apply_security_group(&self, sg: &SecurityGroup, members: &[Port]) -> ApiResult<()> {
        self.apply_security_group_body(sg, members).await
    }
    /// Delete `pg_<sg uuid>`; the database drops its ACLs with it.
    async fn delete_security_group(&self, id: &Id) -> ApiResult<()> {
        self.delete_security_group_body(id).await
    }
    /// `NAT` row on the router that has an interface on the target port's
    /// subnet (`view.router`):
    /// * `type = dnat_and_snat`, `external_ip = <fip address>`,
    ///   `logical_ip = <target fixed ip>`, `logical_port = <port uuid>`,
    ///   `external_mac = <target port mac>` (distributed: the chassis
    ///   hosting the machine answers ARP and forwards directly; a port
    ///   without a mac yet gets neither column and goes through the
    ///   gateway), `options:stateless = false`,
    ///   `external_ids:pistack:id = <fip id>`, `pistack:router = <rtr id>`.
    /// * A matching row writes nothing. A changed target or router replaces
    ///   the row in one transaction.
    /// * Disassociate (`target: None`): remove the NAT row matching
    ///   `external_ids:pistack:id`.
    /// * No router joining the port's subnet to the floating IP's network
    ///   is an `unavailable` error, retried by the reconciler. The message
    ///   names the port and both networks and says what fixes it: attach
    ///   the port's subnet to a router with that external network.
    /// * The address itself is reserved in the external subnet by fabric's
    ///   IPAM; OVN needs nothing for an unassociated FIP.
    async fn associate_fip(&self, view: &FipView) -> ApiResult<()> {
        self.associate_fip_body(view).await
    }
    /// Remove any `NAT` row with `external_ids:pistack:id = <fip id>`.
    async fn delete_fip(&self, id: &Id) -> ApiResult<()> {
        self.delete_fip_body(id).await
    }
}
