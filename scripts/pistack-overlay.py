#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 P. Industries LLC and PiStack contributors
"""Converge one host's overlay datapath from a fabric plan.

    pistack-overlay apply --plan FILE
    pistack-overlay apply --plan -
    pistack-overlay apply --plan FILE --dry-run
    pistack-overlay --self-test

The plan is JSON, version 1. Each network is a bridge plus one VXLAN
device (dstport 4789, nolearning, static FDB), unless `provider` is set.
A provider network has no VXLAN and no FDB. An integer VLAN creates a
`pv` sub-interface of physnet; a null VLAN enslaves physnet and never
deletes it. Each router is its own network namespace. The gateway
address, SNAT, and DNAT live inside that namespace, never on the root
bridge, so two routers can use the same tenant CIDR. A network
`floating` entry is another namespace on this host: the floating
address is a host prefix on the namespace veth, DNAT sends it to the
fixed address, and SNAT rewrites only that fixed address. external_dev
is kept on the plan and never passed to ip.

A router interface may list DHCP leases. dnsmasq for those leases runs
inside that router namespace only. Port, gateway, floating, fixed, DNS,
and route addresses may be IPv4 or IPv6. Underlay and VXLAN endpoints
stay private unicast IPv4. IPv6 packet rules use ip6tables. A router
external address may be IPv4 or a unique-local IPv6 address; SNAT
rewrites each tenant prefix of that same family.

State in $PISTACK_OVERLAY_STATE (default /var/lib/pistack/overlay) lists
the devices, FDB entries, routes, veths, floating addresses, and rules
this tool created. A later apply deletes only the bridge, VXLAN, network
namespace, and veth this tool created for a record the new plan dropped,
plus the FDB entries, rules, routes, and floating addresses that went
with them. A VLAN link and the physical port stay. A second apply
succeeds when those devices are already gone. Chain PISTACK-SG is never
modified. pistack0, wg-fleet, simbr0, an empty name, and any name that
starts with pi are refused.

PISTACK_OVERLAY_IP, PISTACK_OVERLAY_BRIDGE, PISTACK_OVERLAY_IPTABLES,
PISTACK_OVERLAY_IP6TABLES, PISTACK_OVERLAY_SYSCTL, and
PISTACK_OVERLAY_DNSMASQ, when set, are absolute paths to those binaries.
Unset, the bare command name is used so PATH can supply it.
"""

from __future__ import annotations

import argparse
import ipaddress
import io
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from typing import NoReturn, Optional

DEFAULT_STATE_DIR = "/var/lib/pistack/overlay"
ZERO_MAC = "00:00:00:00:00:00"
METADATA_ADDR = "169.254.169.254"
SG_CHAIN = "PISTACK-OVL"
COMMENT_PREFIX = "pistack-ovl-"
VXLAN_PORT = "4789"
PROTOS = ("tcp", "udp", "icmp", "any")

BRIDGE_RE = re.compile(r"^po[0-9a-f]{12}$")
VXLAN_RE = re.compile(r"^vx[0-9a-f]{12}$")
NETNS_RE = re.compile(r"^ns[0-9a-f]{12}$")
VETH_A_RE = re.compile(r"^a[0-9a-f]{12}$")
VETH_B_RE = re.compile(r"^b[0-9a-f]{12}$")
VETH_C_RE = re.compile(r"^c[0-9a-f]{12}$")
VETH_D_RE = re.compile(r"^d[0-9a-f]{12}$")
VETH_E_RE = re.compile(r"^e[0-9a-f]{12}$")
VETH_F_RE = re.compile(r"^f[0-9a-f]{12}$")
VETH_ROOT_RE = re.compile(r"^[ace][0-9a-f]{12}$")
VETH_PEER_RE = re.compile(r"^[bdf][0-9a-f]{12}$")
ROUTE_DEV_RE = re.compile(r"^[cd][0-9a-f]{12}$")
FLOAT_NS_RE = re.compile(r"^nf[0-9a-f]{12}$")
ANY_NS_RE = re.compile(r"^n[sf][0-9a-f]{12}$")
VLAN_LINK_RE = re.compile(r"^pv[0-9a-f]{12}$")
ADDR_DEV_RE = re.compile(r"^f[0-9a-f]{12}$")
DHCP_FILE_RE = re.compile(
    r"^dhcp-(ns[0-9a-f]{12})-(b[0-9a-f]{12})\.(?:conf|pid|leases)$"
)
CREATED_RE = re.compile(
    r"^(?:po[0-9a-f]{12}|vx[0-9a-f]{12}|pv[0-9a-f]{12}|"
    r"ns[0-9a-f]{12}|nf[0-9a-f]{12}|[abcdef][0-9a-f]{12})$"
)
MAC_RE = re.compile(r"^[0-9a-f]{2}(:[0-9a-f]{2}){5}$")
PI_DEV_RE = re.compile(r"^pi[0-9a-fA-F]+$")
SAFE_ID_RE = re.compile(r"^[A-Za-z0-9_.:-]{1,200}$")
RULE_TABLES = ("filter", "nat")
RULE_CHAINS = ("INPUT", "FORWARD", "PREROUTING", "POSTROUTING", SG_CHAIN)
TOOL_ENV = {
    "ip": "PISTACK_OVERLAY_IP",
    "bridge": "PISTACK_OVERLAY_BRIDGE",
    "iptables": "PISTACK_OVERLAY_IPTABLES",
    "ip6tables": "PISTACK_OVERLAY_IP6TABLES",
    "sysctl": "PISTACK_OVERLAY_SYSCTL",
}


def die(msg: str) -> NoReturn:
    print(f"pistack-overlay: {msg}", file=sys.stderr)
    raise SystemExit(2)


def fail(msg: str) -> NoReturn:
    print(f"pistack-overlay: {msg}", file=sys.stderr)
    raise SystemExit(1)


def expect_str(value: object, what: str) -> str:
    if not isinstance(value, str) or not value:
        die(f"{what} must be a string")
    return value


def expect_int(value: object, what: str, lo: int, hi: int) -> int:
    # bool is an int subclass; JSON true must not pass as 1.
    if type(value) is not int:
        die(f"{what} must be an integer")
    if value < lo or value > hi:
        die(f"{what} {value} is outside {lo}..{hi}")
    return value


def expect_bool(value: object, what: str) -> bool:
    if not isinstance(value, bool):
        die(f"{what} must be a boolean")
    return value


def _check_addr(value: object, what: str, versions: tuple[int, ...]) -> str:
    text = expect_str(value, what)
    try:
        ip = ipaddress.ip_address(text)
    except ValueError:
        die(f"{what} {text} is not an address")
    if ip.version not in versions:
        if versions == (4,):
            die(f"{what} must be ipv4")
        die(f"{what} must be ipv4 or ipv6")
    if (
        ip.is_multicast
        or ip.is_unspecified
        or (
            isinstance(ip, ipaddress.IPv4Address)
            and ip == ipaddress.IPv4Address("255.255.255.255")
        )
    ):
        die(f"{what} {text} is not usable")
    if str(ip) != text:
        die(f"{what} {text} is not canonical")
    return text


def _private_underlay(ip: ipaddress.IPv4Address) -> bool:
    """10/8, 172.16/12, 192.168/16, or 100.64/10. Not public, not loopback."""
    if (
        ip.is_multicast
        or ip.is_unspecified
        or ip.is_loopback
        or ip == ipaddress.IPv4Address("255.255.255.255")
    ):
        return False
    a, b, _, _ = ip.packed
    return (
        a == 10
        or (a == 172 and 16 <= b <= 31)
        or (a == 192 and b == 168)
        or (a == 100 and 64 <= b <= 127)
    )


def check_ip(value: object, what: str) -> str:
    """Underlay and VXLAN endpoints stay private unicast IPv4."""
    text = _check_addr(value, what, (4,))
    if not _private_underlay(ipaddress.IPv4Address(text)):
        die(f"{what} {text} is not a private underlay")
    return text


def check_addr(value: object, what: str) -> str:
    return _check_addr(value, what, (4, 6))


def check_mac(value: object, what: str) -> str:
    text = expect_str(value, what).lower()
    if not MAC_RE.fullmatch(text):
        die(f"{what} {value} is not a mac address")
    if text == ZERO_MAC:
        die(f"{what} must not be {ZERO_MAC}")
    return text


def check_name(value: object, pattern: re.Pattern[str], what: str) -> str:
    text = expect_str(value, what)
    if (
        text in ("pistack0", "wg-fleet", "simbr0")
        or text.startswith("pi")
        or not pattern.fullmatch(text)
    ):
        die(f"{what} {text} is not allowed")
    return text


def check_cidr(value: object, what: str) -> str:
    text = expect_str(value, what)
    try:
        net = ipaddress.ip_network(text, strict=True)
    except ValueError:
        die(f"{what} {text} is not a network")
    return str(net)


def addr_version(addr: str) -> int:
    return ipaddress.ip_address(addr.split("/")[0]).version


def _is_ula(addr: str) -> bool:
    """Unique-local IPv6, fc00::/7."""
    ip = ipaddress.ip_address(addr.split("/")[0])
    return isinstance(ip, ipaddress.IPv6Address) and (ip.packed[0] & 0xFE) == 0xFC


def family_opt(version: int) -> str:
    return "-4" if version == 4 else "-6"


def host_cidr(addr: str) -> str:
    ip = ipaddress.ip_address(addr)
    return f"{addr}/{ip.max_prefixlen}"


def prefix_for(cidr: str, value: object) -> int:
    version = ipaddress.ip_network(cidr).version
    if version == 4:
        return expect_int(value, "prefix", 1, 30)
    return expect_int(value, "prefix", 1, 128)


def same_version(left: str, right: str, what: str) -> None:
    if addr_version(left) != addr_version(right):
        die(what)


def check_physnet(value: object) -> str:
    text = expect_str(value, "physnet")
    if (
        text in ("pistack0", "wg-fleet", "simbr0")
        or text.startswith("pi")
        or any(ch.isspace() for ch in text)
        or "/" in text
    ):
        die(f"physnet {text} is not allowed")
    return text


def check_id(value: object, what: str) -> str:
    text = expect_str(value, what)
    if not SAFE_ID_RE.fullmatch(text):
        die(f"{what} {text!r} is not a safe id")
    return text


def note_external_dev(value: object) -> Optional[str]:
    """Keep the planner's underlay name. Callers must not pass it to ip."""
    if value is None:
        return None
    text = expect_str(value, "external_dev")
    if any(ch.isspace() for ch in text) or "/" in text:
        die("external_dev is not a device name")
    return text


def optional_port(value: object, what: str) -> Optional[int]:
    if value is None:
        return None
    return expect_int(value, what, 1, 65535)


@dataclass
class SgRule:
    proto: str
    port_min: Optional[int]
    port_max: Optional[int]
    cidr: Optional[str]


@dataclass
class Port:
    id: str
    mac: str
    address: str
    underlay: str
    ingress: list[SgRule]
    egress: list[SgRule]


@dataclass
class DhcpRoute:
    destination: str
    nexthop: str


@dataclass
class DhcpLease:
    mac: str
    address: str
    dns: list[str]
    routes: list[DhcpRoute]


@dataclass
class Iface:
    network_id: str
    veth_bridge: str
    veth_ns: str
    gateway: str
    prefix: int
    cidr: str
    dhcp: list[DhcpLease]


@dataclass
class Dnat:
    id: str
    floating: str
    fixed: str


@dataclass
class Floating:
    id: str
    floating: str
    fixed: str
    netns: str
    veth_bridge: str
    veth_ns: str


@dataclass
class Provider:
    physnet: str
    vlan: Optional[int]
    link: Optional[str]


@dataclass
class Router:
    id: str
    netns: str
    interfaces: list[Iface]
    external_veth_root: Optional[str]
    external_veth_ns: Optional[str]
    external_ip: Optional[str]
    external_dev: Optional[str]
    snat: bool
    dnat: list[Dnat]
    metadata: bool


@dataclass
class Network:
    id: str
    vni: int
    mtu: int
    bridge: str
    vxlan: str
    local: str
    peers: list[str]
    ports: list[Port]
    routers: list[Router]
    metadata: bool
    provider: Optional[Provider]
    floating: list[Floating]


@dataclass
class Plan:
    host: str
    networks: list[Network]

    def by_id(self, network_id: str) -> Network:
        for net in self.networks:
            if net.id == network_id:
                return net
        die(f"network {network_id} is not in the plan")


@dataclass
class State:
    created: set[str]
    fdb: list[tuple[str, str, str]]
    routes: list[tuple[str, str, str, str]]
    veths: list[tuple[str, str, str]]
    rules: list[tuple[str, str, str, tuple[str, ...]]]
    addrs: list[tuple[str, str, str]]


def empty_state() -> State:
    return State(set(), [], [], [], [], [])


def require_mapping(value: object, what: str) -> dict:
    if not isinstance(value, dict):
        die(f"{what} must be an object")
    return value


def require_list(value: object, what: str) -> list:
    if not isinstance(value, list):
        die(f"{what} must be a list")
    return value


def parse_sg_rule(item: object, what: str) -> SgRule:
    raw = require_mapping(item, what)
    proto = expect_str(raw.get("proto"), f"{what} proto")
    if proto not in PROTOS:
        die(f"{what} proto {proto} is not allowed")
    cidr: Optional[str] = None
    if "cidr" in raw and raw.get("cidr") is not None:
        cidr = check_cidr(raw.get("cidr"), f"{what} cidr")
    # icmp and any do not match ports. The planner still emits numbers.
    if proto in ("icmp", "any"):
        return SgRule(proto, None, None, cidr)
    port_min = optional_port(raw.get("port_min"), f"{what} port_min")
    port_max = optional_port(raw.get("port_max"), f"{what} port_max")
    if (port_min is None) != (port_max is None):
        die(f"{what} needs both port_min and port_max")
    if port_min is not None and port_max is not None and port_min > port_max:
        die(f"{what} port_min is greater than port_max")
    if port_min == 0 or port_max == 0:
        port_min = None
        port_max = None
    return SgRule(proto, port_min, port_max, cidr)


def parse_sg_list(value: object, what: str) -> list[SgRule]:
    return [parse_sg_rule(item, what) for item in require_list(value, what)]


def _family_of_cidr(cidr: str) -> int:
    return ipaddress.ip_network(cidr).version


def parse_port(item: object) -> Port:
    raw = require_mapping(item, "port")
    address = check_addr(raw.get("address"), "port address")
    port = Port(
        id=check_id(raw.get("id"), "port id"),
        mac=check_mac(raw.get("mac"), "port mac"),
        address=address,
        underlay=check_ip(raw.get("underlay"), "port underlay"),
        ingress=parse_sg_list(raw.get("ingress"), "ingress"),
        egress=parse_sg_list(raw.get("egress"), "egress"),
    )
    fam = addr_version(address)
    for rule in port.ingress + port.egress:
        if rule.cidr is not None and _family_of_cidr(rule.cidr) != fam:
            die("cidr family does not match port address")
    return port


def parse_dhcp_route(item: object, what: str) -> DhcpRoute:
    raw = require_mapping(item, what)
    destination = check_cidr(raw.get("destination"), f"{what} destination")
    nexthop = check_addr(raw.get("nexthop"), f"{what} nexthop")
    if _family_of_cidr(destination) != addr_version(nexthop):
        die(f"{what} nexthop family does not match destination")
    return DhcpRoute(destination, nexthop)


def parse_dhcp_lease(item: object) -> DhcpLease:
    raw = require_mapping(item, "dhcp")
    dns_raw = raw.get("dns", [])
    if dns_raw is None:
        dns_raw = []
    routes_raw = raw.get("routes", [])
    if routes_raw is None:
        routes_raw = []
    dns = [check_addr(ip, "dhcp dns") for ip in require_list(dns_raw, "dhcp dns")]
    routes = [
        parse_dhcp_route(route, "dhcp route")
        for route in require_list(routes_raw, "dhcp routes")
    ]
    return DhcpLease(
        mac=check_mac(raw.get("mac"), "dhcp mac"),
        address=check_addr(raw.get("address"), "dhcp address"),
        dns=dns,
        routes=routes,
    )


def parse_dhcp_list(value: object) -> list[DhcpLease]:
    if value is None:
        return []
    leases = [parse_dhcp_lease(item) for item in require_list(value, "dhcp")]
    macs: set[str] = set()
    addrs: set[str] = set()
    for lease in leases:
        if lease.mac in macs:
            die(f"dhcp mac {lease.mac} is duplicated")
        macs.add(lease.mac)
        if lease.address in addrs:
            die(f"dhcp address {lease.address} is duplicated")
        addrs.add(lease.address)
    return leases


def parse_iface(item: object) -> Iface:
    raw = require_mapping(item, "interface")
    cidr = check_cidr(raw.get("cidr"), "cidr")
    gateway = check_addr(raw.get("gateway"), "gateway")
    if addr_version(gateway) != _family_of_cidr(cidr):
        die("gateway family does not match cidr")
    return Iface(
        network_id=check_id(raw.get("network_id"), "interface network_id"),
        veth_bridge=check_name(raw.get("veth_bridge"), VETH_A_RE, "veth_bridge"),
        veth_ns=check_name(raw.get("veth_ns"), VETH_B_RE, "veth_ns"),
        gateway=gateway,
        prefix=prefix_for(cidr, raw.get("prefix")),
        cidr=cidr,
        dhcp=parse_dhcp_list(raw.get("dhcp", [])),
    )


def parse_dnat(item: object) -> Dnat:
    raw = require_mapping(item, "dnat")
    floating = check_addr(raw.get("floating"), "floating")
    fixed = check_addr(raw.get("fixed"), "fixed")
    same_version(floating, fixed, "floating and fixed must be the same family")
    return Dnat(
        id=check_id(raw.get("id"), "dnat id"),
        floating=floating,
        fixed=fixed,
    )


def parse_optional_ip(value: object, what: str) -> Optional[str]:
    if value is None:
        return None
    return check_addr(value, what)


def parse_router(item: object) -> Router:
    raw = require_mapping(item, "router")
    external_ip = parse_optional_ip(raw.get("external_ip"), "external_ip")
    root = raw.get("external_veth_root")
    peer = raw.get("external_veth_ns")
    if external_ip is None:
        ext_root = None
        ext_ns = None
    else:
        ext_root = check_name(root, VETH_C_RE, "external_veth_root")
        ext_ns = check_name(peer, VETH_D_RE, "external_veth_ns")
    router = Router(
        id=check_id(raw.get("id"), "router id"),
        netns=check_name(raw.get("netns"), NETNS_RE, "netns"),
        interfaces=[parse_iface(iface) for iface in require_list(raw.get("interfaces"), "interfaces")],
        external_veth_root=ext_root,
        external_veth_ns=ext_ns,
        external_ip=external_ip,
        external_dev=note_external_dev(raw.get("external_dev")),
        snat=expect_bool(raw.get("snat"), "snat"),
        dnat=[parse_dnat(rule) for rule in require_list(raw.get("dnat"), "dnat")],
        metadata=expect_bool(raw.get("metadata"), "metadata"),
    )
    if router.external_ip is None and (router.snat or router.dnat):
        die(f"router {router.id} snat or dnat needs external_ip")
    if (
        router.external_ip is not None
        and addr_version(router.external_ip) == 6
        and not _is_ula(router.external_ip)
    ):
        die(f"router {router.id} external_ip is not a private ipv6 address")
    if router.external_ip is not None:
        external_fam = addr_version(router.external_ip)
        for rule in router.dnat:
            if addr_version(rule.floating) != external_fam:
                die(f"router {router.id} floating family does not match external_ip")
        if router.snat:
            for iface in router.interfaces:
                if _family_of_cidr(iface.cidr) != external_fam:
                    die(f"router {router.id} snat family does not match external_ip")
    return router


def parse_floating(item: object) -> Floating:
    raw = require_mapping(item, "floating")
    floating = check_addr(raw.get("floating"), "floating")
    fixed = check_addr(raw.get("fixed"), "fixed")
    same_version(floating, fixed, "floating and fixed must be the same family")
    return Floating(
        id=check_id(raw.get("id"), "floating id"),
        floating=floating,
        fixed=fixed,
        netns=check_name(raw.get("netns"), FLOAT_NS_RE, "floating netns"),
        veth_bridge=check_name(raw.get("veth_bridge"), VETH_E_RE, "floating veth_bridge"),
        veth_ns=check_name(raw.get("veth_ns"), VETH_F_RE, "floating veth_ns"),
    )


def parse_provider(value: object) -> Optional[Provider]:
    if value is None:
        return None
    raw = require_mapping(value, "provider")
    physnet = check_physnet(raw.get("physnet"))
    vlan_raw = raw.get("vlan")
    if vlan_raw is None:
        return Provider(physnet, None, None)
    vlan = expect_int(vlan_raw, "vlan", 1, 4094)
    link = check_name(raw.get("link"), VLAN_LINK_RE, "provider link")
    return Provider(physnet, vlan, link)


def parse_network(item: object) -> Network:
    raw = require_mapping(item, "network")
    peers: list[str] = []
    for peer in require_list(raw.get("peers"), "peers"):
        ip = check_ip(peer, "peer")
        if ip in peers:
            die(f"peer {ip} is duplicated")
        peers.append(ip)
    ports = [parse_port(port) for port in require_list(raw.get("ports"), "ports")]
    raw_meta = raw.get("metadata", False)
    if not isinstance(raw_meta, bool):
        die("metadata must be a boolean")
    floating_raw = raw.get("floating", [])
    if floating_raw is None:
        floating_raw = []
    return Network(
        id=check_id(raw.get("id"), "network id"),
        vni=expect_int(raw.get("vni"), "vni", 1, 16777215),
        mtu=expect_int(raw.get("mtu"), "mtu", 1280, 1500),
        bridge=check_name(raw.get("bridge"), BRIDGE_RE, "bridge"),
        vxlan=check_name(raw.get("vxlan"), VXLAN_RE, "vxlan"),
        local=check_ip(raw.get("local"), "local"),
        peers=peers,
        ports=ports,
        routers=[parse_router(router) for router in require_list(raw.get("routers"), "routers")],
        metadata=raw_meta,
        provider=parse_provider(raw.get("provider")),
        floating=[parse_floating(item) for item in require_list(floating_raw, "floating")],
    )


def take_unique(seen: set[str], name: str, what: str) -> None:
    if name in seen:
        die(f"{what} {name} is duplicated")
    seen.add(name)


def check_plan_unique(plan: Plan) -> None:
    names: set[str] = set()
    vnis: set[int] = set()
    net_ids: set[str] = set()
    router_ids: set[str] = set()
    external_ips: set[str] = set()
    floatings: set[str] = set()
    floating_ids: set[str] = set()
    physnets: set[tuple[str, Optional[int]]] = set()
    every_external = {
        router.external_ip
        for net in plan.networks
        for router in net.routers
        if router.external_ip is not None
    }
    for net in plan.networks:
        take_unique(net_ids, net.id, "network id")
        if net.vni in vnis:
            die(f"vni {net.vni} is duplicated")
        vnis.add(net.vni)
        for name, what in (
            (net.bridge, "bridge"),
            (net.vxlan, "vxlan"),
        ):
            take_unique(names, name, what)
        if net.provider is not None:
            if net.provider.link is not None:
                take_unique(names, net.provider.link, "provider link")
            phys = (net.provider.physnet, net.provider.vlan)
            if phys in physnets:
                die(f"physnet {net.provider.physnet} vlan {net.provider.vlan} is duplicated")
            physnets.add(phys)
        if plan.host in net.peers or net.local in net.peers:
            die(f"peers of {net.id} contain this host")
        macs: set[str] = set()
        for port in net.ports:
            if port.mac in macs:
                die(f"mac {port.mac} is duplicated on {net.id}")
            macs.add(port.mac)
        for entry in net.floating:
            take_unique(floating_ids, entry.id, "floating id")
            take_unique(names, entry.netns, "floating netns")
            take_unique(names, entry.veth_bridge, "floating veth_bridge")
            take_unique(names, entry.veth_ns, "floating veth_ns")
            if entry.floating in floatings:
                die(f"floating {entry.floating} is duplicated")
            if entry.floating in every_external:
                die(f"floating {entry.floating} collides with external_ip")
            floatings.add(entry.floating)
        for router in net.routers:
            take_unique(router_ids, router.id, "router id")
            take_unique(names, router.netns, "netns")
            if router.external_ip is not None:
                if router.external_ip in external_ips:
                    die(f"external_ip {router.external_ip} is duplicated")
                external_ips.add(router.external_ip)
                take_unique(names, router.external_veth_root or "", "external_veth_root")
                take_unique(names, router.external_veth_ns or "", "external_veth_ns")
            for rule in router.dnat:
                if rule.floating in floatings:
                    die(f"floating {rule.floating} is duplicated")
                floatings.add(rule.floating)
                if rule.floating == router.external_ip:
                    die(f"floating {rule.floating} collides with external_ip")
            for iface in router.interfaces:
                take_unique(names, iface.veth_bridge, "veth_bridge")
                take_unique(names, iface.veth_ns, "veth_ns")
                plan.by_id(iface.network_id)


def parse_plan(data: object) -> Plan:
    raw = require_mapping(data, "plan")
    version = raw.get("version")
    if type(version) is not int or version != 1:
        die("version must be 1")
    plan = Plan(
        host=check_ip(raw.get("host"), "host"),
        networks=[parse_network(item) for item in require_list(raw.get("networks"), "networks")],
    )
    check_plan_unique(plan)
    return plan


def state_dir() -> str:
    return os.environ.get("PISTACK_OVERLAY_STATE", DEFAULT_STATE_DIR)


def state_path() -> str:
    return os.path.join(state_dir(), "state.json")


def _expect_stored_name(value: object, pattern: re.Pattern[str], what: str) -> str:
    if not isinstance(value, str) or not pattern.fullmatch(value):
        die(f"state {what} {value!r} is not allowed")
    if value == "pistack0" or value.startswith("pi"):
        die(f"state {what} {value} is not allowed")
    return value


def _load_fdb(raw: object) -> list[tuple[str, str, str]]:
    found: list[tuple[str, str, str]] = []
    for item in require_list(raw, "state fdb"):
        rec = require_mapping(item, "state fdb entry")
        dev = _expect_stored_name(rec.get("dev"), VXLAN_RE, "fdb dev")
        mac = check_mac(rec.get("mac"), "state fdb mac") if rec.get("mac") != ZERO_MAC else ZERO_MAC
        if mac != ZERO_MAC and not MAC_RE.fullmatch(mac):
            die("state fdb mac is not allowed")
        dst = check_ip(rec.get("dst"), "state fdb dst")
        found.append((dev, mac, dst))
    return found


def _route_fam(dest: str, raw: object) -> str:
    if raw in ("4", "6"):
        return str(raw)
    if isinstance(raw, str):
        die(f"state route fam {raw} is not allowed")
    if dest != "default" and ":" in dest:
        return "6"
    return "4"


def _load_routes(raw: object) -> list[tuple[str, str, str, str]]:
    found: list[tuple[str, str, str, str]] = []
    for item in require_list(raw, "state routes"):
        rec = require_mapping(item, "state route")
        dest = expect_str(rec.get("dest"), "state route dest")
        if dest != "default":
            check_cidr(dest, "state route dest")
        dev = _expect_stored_name(rec.get("dev"), ROUTE_DEV_RE, "route dev")
        netns_raw = rec.get("netns")
        netns = "" if netns_raw in (None, "") else _expect_stored_name(netns_raw, ANY_NS_RE, "route netns")
        found.append((dest, dev, netns, _route_fam(dest, rec.get("fam"))))
    return found


def _pair_ok(root: str, peer: str) -> bool:
    return (root[0], peer[0]) in {("a", "b"), ("c", "d"), ("e", "f")}


def _load_veths(raw: object) -> list[tuple[str, str, str]]:
    found: list[tuple[str, str, str]] = []
    for item in require_list(raw, "state veths"):
        rec = require_mapping(item, "state veth")
        root = _expect_stored_name(rec.get("root"), VETH_ROOT_RE, "veth root")
        peer = _expect_stored_name(rec.get("peer"), VETH_PEER_RE, "veth peer")
        netns = _expect_stored_name(rec.get("netns"), ANY_NS_RE, "veth netns")
        if not _pair_ok(root, peer):
            die(f"state veth pair {root}/{peer} is not allowed")
        found.append((root, peer, netns))
    return found


def _load_rules(raw: object) -> list[tuple[str, str, str, tuple[str, ...]]]:
    found = []
    for item in require_list(raw, "state rules"):
        rec = require_mapping(item, "state rule")
        netns_raw = rec.get("netns")
        netns = "" if netns_raw in (None, "") else _expect_stored_name(netns_raw, ANY_NS_RE, "rule netns")
        table = expect_str(rec.get("table"), "state rule table")
        chain = expect_str(rec.get("chain"), "state rule chain")
        if table not in RULE_TABLES or chain not in RULE_CHAINS or chain == "PISTACK-SG":
            die(f"state rule {table}/{chain} is not allowed")
        spec_raw = require_list(rec.get("spec"), "state rule spec")
        spec: list[str] = []
        for tok in spec_raw:
            if not isinstance(tok, str) or not tok or any(ch in tok for ch in "\n\r\x00"):
                die("state rule spec is corrupt")
            if tok in ("pistack0", "wg-fleet", "simbr0", "PISTACK-SG") or (
                tok.startswith("pi") and not tok.startswith(COMMENT_PREFIX)
            ):
                die(f"state rule names {tok}")
            if tok == "0.0.0.0" or tok.startswith("0.0.0.0/"):
                die("state rule uses 0.0.0.0")
            spec.append(tok)
        found.append((netns, table, chain, tuple(spec)))
    return found


def _load_addrs(raw: object) -> list[tuple[str, str, str]]:
    found: list[tuple[str, str, str]] = []
    for item in require_list(raw, "state addrs"):
        rec = require_mapping(item, "state addr")
        text = expect_str(rec.get("addr"), "state addr")
        try:
            iface = ipaddress.ip_interface(text)
        except ValueError:
            die(f"state addr {text} is not allowed")
        if iface.network.prefixlen != iface.max_prefixlen or str(iface) != text:
            die(f"state addr {text} is not a host prefix")
        if iface.ip.is_multicast or iface.ip.is_unspecified:
            die(f"state addr {text} is not usable")
        dev = _expect_stored_name(rec.get("dev"), ADDR_DEV_RE, "addr dev")
        netns = _expect_stored_name(rec.get("netns"), FLOAT_NS_RE, "addr netns")
        found.append((text, dev, netns))
    return found


def load_state() -> State:
    path = state_path()
    if not os.path.exists(path):
        return empty_state()
    try:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, json.JSONDecodeError) as exc:
        die(f"cannot read {path}: {exc}")
    raw = require_mapping(data, "state")
    if raw.get("version") != 1:
        die(f"{path} version must be 1")
    created: set[str] = set()
    for name in require_list(raw.get("created", []), "state created"):
        created.add(_expect_stored_name(name, CREATED_RE, "created"))
    return State(
        created=created,
        fdb=_load_fdb(raw.get("fdb", [])),
        routes=_load_routes(raw.get("routes", [])),
        veths=_load_veths(raw.get("veths", [])),
        rules=_load_rules(raw.get("rules", [])),
        addrs=_load_addrs(raw.get("addrs", [])),
    )


def save_state(state: State) -> None:
    os.makedirs(state_dir(), mode=0o755, exist_ok=True)
    path = state_path()
    payload = {
        "version": 1,
        "created": sorted(state.created),
        "fdb": [{"dev": dev, "mac": mac, "dst": dst} for dev, mac, dst in state.fdb],
        "routes": [
            {"dest": dest, "dev": dev, "netns": netns or None, "fam": fam}
            for dest, dev, netns, fam in state.routes
        ],
        "addrs": [
            {"addr": addr, "dev": dev, "netns": netns} for addr, dev, netns in state.addrs
        ],
        "veths": [
            {"root": root, "peer": peer, "netns": netns} for root, peer, netns in state.veths
        ],
        "rules": [
            {"netns": netns or None, "table": table, "chain": chain, "spec": list(spec)}
            for netns, table, chain, spec in state.rules
        ],
    }
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump(payload, fh, indent=2)
        fh.write("\n")
    os.chmod(tmp, 0o644)
    os.replace(tmp, path)


def resolve_tool(bare: str) -> str:
    env_name = TOOL_ENV[bare]
    override = os.environ.get(env_name)
    if not override:
        return bare
    if not os.path.isabs(override):
        die(f"{env_name} must be an absolute path")
    return override


class Runner:
    """ip, bridge, iptables, ip6tables, and sysctl. Dry-run prints mutations only."""

    def __init__(self, dry: bool) -> None:
        self.dry = dry
        self.ip = resolve_tool("ip")
        self.bridge = resolve_tool("bridge")
        self.iptables = resolve_tool("iptables")
        self.ip6tables = resolve_tool("ip6tables")
        self.sysctl = resolve_tool("sysctl")
        self._comment: dict[str, bool] = {}

    def guard(self, argv: list[str]) -> None:
        for tok in argv[1:]:
            if (
                tok == ""
                or tok in ("pistack0", "wg-fleet", "simbr0", "PISTACK-SG")
                or (tok.startswith("pi") and not tok.startswith(COMMENT_PREFIX))
            ):
                die(f"refusing to touch {tok or 'empty name'}")
            if "all.route_localnet" in tok:
                die("refusing global route_localnet")
            if tok == "0.0.0.0" or tok.startswith("0.0.0.0/"):
                die("refusing unspecified address")

    def _exec(self, argv: list[str]) -> subprocess.CompletedProcess[str]:
        try:
            return subprocess.run(argv, check=False, capture_output=True, text=True)
        except OSError as exc:
            fail(f"cannot run {argv[0]}: {exc}")

    def _boom(self, argv: list[str], proc: subprocess.CompletedProcess[str]) -> NoReturn:
        err = (proc.stderr or proc.stdout or "").strip()
        if len(err) > 400:
            err = err[:400] + "..."
        suffix = f": {err}" if err else ""
        fail(f"command failed ({proc.returncode}): {shlex.join(argv)}{suffix}")

    def mutate(self, argv: list[str]) -> None:
        self.guard(argv)
        if self.dry:
            print(shlex.join(argv))
            return
        proc = self._exec(argv)
        if proc.returncode != 0:
            self._boom(argv, proc)

    def mutate_create(self, argv: list[str]) -> bool:
        """Create. True if this call created it. File-exists is success and False."""
        self.guard(argv)
        if self.dry:
            print(shlex.join(argv))
            return True
        proc = self._exec(argv)
        if proc.returncode == 0:
            return True
        err = ((proc.stderr or "") + (proc.stdout or "")).lower()
        if "file exists" in err or "already exists" in err:
            return False
        self._boom(argv, proc)

    def mutate_missing(self, argv: list[str], extra_ok: tuple[str, ...] = ()) -> None:
        """Run a delete. Missing objects are already the desired state."""
        self.guard(argv)
        if self.dry:
            print(shlex.join(argv))
            return
        proc = self._exec(argv)
        if proc.returncode == 0:
            return
        err = ((proc.stderr or "") + (proc.stdout or "")).lower()
        phrases = ("no such", "cannot find", "does not exist", "not exist", *extra_ok)
        if any(part in err for part in phrases):
            return
        self._boom(argv, proc)

    def probe(self, argv: list[str]) -> tuple[int, str, str]:
        self.guard(argv)
        proc = self._exec(argv)
        return proc.returncode, proc.stdout or "", proc.stderr or ""

    def link_exists(self, name: str) -> bool:
        rc, _, _ = self.probe([self.ip, "link", "show", "dev", name])
        return rc == 0

    def link_in_netns(self, netns: str, name: str) -> bool:
        rc, _, _ = self.probe(
            [self.ip, "netns", "exec", netns, self.ip, "link", "show", "dev", name]
        )
        return rc == 0

    def netns_names(self) -> set[str]:
        rc, out, err = self.probe([self.ip, "netns", "list"])
        if rc != 0:
            text = err.strip() or out.strip()
            fail(f"ip netns list failed ({rc})" + (f": {text}" if text else ""))
        found = set()
        for line in out.splitlines():
            parts = line.split()
            if parts:
                found.add(parts[0])
        return found

    def comments_for(self, v6: bool) -> bool:
        key = "6" if v6 else "4"
        cached = self._comment.get(key)
        if cached is not None:
            return cached
        tool = self.ip6tables if v6 else self.iptables
        rc, out, err = self.probe([tool, "-m", "comment", "-h"])
        if rc == 0:
            self._comment[key] = True
            return True
        text = (out + err).lower()
        broken = ("couldn't", "could not", "unrecognized", "no such", "not found", "error")
        ok = "comment" in text and not any(part in text for part in broken)
        self._comment[key] = ok
        return ok

    def comments(self) -> bool:
        return self.comments_for(False)

    def rule_tool(self, v6: bool) -> str:
        return self.ip6tables if v6 else self.iptables

    def rule_argv(
        self,
        netns: str,
        table: str,
        op: str,
        chain: str,
        spec: tuple[str, ...],
        v6: Optional[bool] = None,
    ) -> list[str]:
        if v6 is None:
            v6 = spec_v6(spec)
        cmd = [self.rule_tool(v6), "-t", table, op, chain, *spec]
        if netns:
            return [self.ip, "netns", "exec", netns, *cmd]
        return cmd

    def rule_present(self, netns: str, table: str, chain: str, spec: tuple[str, ...]) -> bool:
        rc, _, _ = self.probe(self.rule_argv(netns, table, "-C", chain, spec))
        return rc == 0


def finish_spec(match: list[str], comment: Optional[str], jump: list[str]) -> tuple[str, ...]:
    spec = list(match)
    if comment:
        spec += ["-m", "comment", "--comment", comment]
    spec += jump
    return tuple(spec)


def port_span(rule: SgRule) -> Optional[str]:
    if rule.port_min is None or rule.port_max is None:
        return None
    if rule.port_min == rule.port_max:
        return str(rule.port_min)
    return f"{rule.port_min}:{rule.port_max}"


def token_is_v6(tok: str) -> bool:
    if ":" not in tok:
        return False
    base = tok.split("/")[0]
    if base.startswith("[") and base.endswith("]"):
        base = base[1:-1]
    try:
        return isinstance(ipaddress.ip_address(base), ipaddress.IPv6Address)
    except ValueError:
        return False


def spec_v6(spec: tuple[str, ...]) -> bool:
    return any(token_is_v6(tok) for tok in spec)


def sg_match(bridge: str, address: str, direction: str, rule: Optional[SgRule]) -> list[str]:
    version = addr_version(address)
    host = host_cidr(address)
    if direction == "in":
        spec = ["-i", bridge, "-d", host]
    else:
        spec = ["-o", bridge, "-s", host]
    if rule is not None and rule.proto in ("tcp", "udp"):
        spec += ["-p", rule.proto, "-m", rule.proto]
        span = port_span(rule)
        if span is not None:
            spec += ["--dport", span]
    elif rule is not None and rule.proto == "icmp":
        # icmp and any never match ports. IPv6 uses the ipv6-icmp protocol name.
        spec += ["-p", "ipv6-icmp" if version == 6 else "icmp"]
    if rule is not None and rule.cidr:
        spec += ["-s", rule.cidr] if direction == "in" else ["-d", rule.cidr]
    return spec


def sg_rules_for_port(bridge: str, port: Port, comment_on: bool) -> list[tuple[str, ...]]:
    """Accept listed flows. A non-empty direction ends in DROP for the rest.

    ESTABLISHED,RELATED is accepted first so the ingress DROP does not cut
    replies to flows the guest opened.
    """
    comment = f"{COMMENT_PREFIX}{port.id}" if comment_on else None
    host = host_cidr(port.address)
    specs: list[tuple[str, ...]] = []
    if port.ingress:
        specs.append(
            finish_spec(
                ["-i", bridge, "-d", host, "-m", "conntrack", "--ctstate", "ESTABLISHED,RELATED"],
                comment,
                ["-j", "ACCEPT"],
            )
        )
        for rule in port.ingress:
            specs.append(finish_spec(sg_match(bridge, port.address, "in", rule), comment, ["-j", "ACCEPT"]))
        specs.append(finish_spec(["-i", bridge, "-d", host], comment, ["-j", "DROP"]))
    if port.egress:
        specs.append(
            finish_spec(
                ["-o", bridge, "-s", host, "-m", "conntrack", "--ctstate", "ESTABLISHED,RELATED"],
                comment,
                ["-j", "ACCEPT"],
            )
        )
        for rule in port.egress:
            specs.append(finish_spec(sg_match(bridge, port.address, "out", rule), comment, ["-j", "ACCEPT"]))
        specs.append(
            finish_spec(
                ["-o", bridge, "-s", host, "-m", "conntrack", "--ctstate", "NEW"],
                comment,
                ["-j", "DROP"],
            )
        )
    return specs


def metadata_rules(bridge: str, comment_on: bool) -> list[tuple[str, str, str, tuple[str, ...]]]:
    comment = f"{COMMENT_PREFIX}metadata" if comment_on else None
    dnat = finish_spec(
        ["-i", bridge, "-d", METADATA_ADDR, "-p", "tcp", "--dport", "80"],
        comment,
        ["-j", "DNAT", "--to-destination", "127.0.0.1:18880"],
    )
    inbound = finish_spec(
        ["-i", bridge, "-p", "tcp", "--dport", "18880"],
        comment,
        ["-j", "ACCEPT"],
    )
    return [
        ("", "nat", "PREROUTING", dnat),
        ("", "filter", "INPUT", inbound),
    ]


@dataclass
class Desired:
    fdb: list[tuple[str, str, str]]
    routes: list[tuple[str, str, str, str]]
    veths: list[tuple[str, str, str]]
    rules: list[tuple[str, str, str, tuple[str, ...]]]
    metadata_bridges: list[str]
    addrs: list[tuple[str, str, str]]


def local_underlay(underlay: str, host: str, local: str) -> bool:
    return underlay == host or underlay == local


def plan_uses_v6(plan: Plan) -> bool:
    for net in plan.networks:
        for port in net.ports:
            if addr_version(port.address) == 6:
                return True
        for entry in net.floating:
            if addr_version(entry.floating) == 6:
                return True
        for router in net.routers:
            if router.external_ip is not None and addr_version(router.external_ip) == 6:
                return True
            for iface in router.interfaces:
                if addr_version(iface.gateway) == 6:
                    return True
            for rule in router.dnat:
                if addr_version(rule.floating) == 6:
                    return True
    return False


def build_desired(plan: Plan, comment_v4: bool, comment_v6: bool) -> Desired:
    fdb: list[tuple[str, str, str]] = []
    routes: list[tuple[str, str, str, str]] = []
    veths: list[tuple[str, str, str]] = []
    rules: list[tuple[str, str, str, tuple[str, ...]]] = []
    addrs: list[tuple[str, str, str]] = []
    meta: list[str] = []
    seen_fdb: set[tuple[str, str, str]] = set()
    seen_meta: set[str] = set()

    def marked(address: str, ident: str) -> Optional[str]:
        on = comment_v6 if addr_version(address) == 6 else comment_v4
        if not on:
            return None
        return f"{COMMENT_PREFIX}{ident}"

    def add_fdb(dev: str, mac: str, dst: str) -> None:
        item = (dev, mac, dst)
        if item in seen_fdb or dst == "0.0.0.0":
            return
        seen_fdb.add(item)
        fdb.append(item)

    for net in plan.networks:
        # Provider networks are bridged onto a physical segment. No VXLAN, no FDB.
        if net.provider is None:
            for peer in net.peers:
                add_fdb(net.vxlan, ZERO_MAC, peer)
            for port in net.ports:
                if not local_underlay(port.underlay, plan.host, net.local):
                    add_fdb(net.vxlan, port.mac, port.underlay)
        for port in net.ports:
            comment_on = comment_v6 if addr_version(port.address) == 6 else comment_v4
            for spec in sg_rules_for_port(net.bridge, port, comment_on):
                rules.append(("", "filter", SG_CHAIN, spec))
        if net.metadata and net.bridge not in seen_meta:
            seen_meta.add(net.bridge)
            meta.append(net.bridge)
            rules.extend(metadata_rules(net.bridge, comment_v4))
        for entry in net.floating:
            veths.append((entry.veth_bridge, entry.veth_ns, entry.netns))
            addrs.append((host_cidr(entry.floating), entry.veth_ns, entry.netns))
            mark = marked(entry.floating, entry.id)
            rules.append(
                (
                    entry.netns,
                    "nat",
                    "PREROUTING",
                    finish_spec(
                        ["-d", host_cidr(entry.floating)],
                        mark,
                        ["-j", "DNAT", "--to-destination", entry.fixed],
                    ),
                )
            )
            # SNAT that fixed host only, never the tenant subnet.
            rules.append(
                (
                    entry.netns,
                    "nat",
                    "POSTROUTING",
                    finish_spec(
                        ["-s", host_cidr(entry.fixed)],
                        mark,
                        ["-j", "SNAT", "--to-source", entry.floating],
                    ),
                )
            )
        for router in net.routers:
            for iface in router.interfaces:
                veths.append((iface.veth_bridge, iface.veth_ns, router.netns))
                home = plan.by_id(iface.network_id)
                if router.metadata and home.bridge not in seen_meta:
                    seen_meta.add(home.bridge)
                    meta.append(home.bridge)
                    rules.extend(metadata_rules(home.bridge, comment_v4))
            if router.external_ip and router.external_veth_root and router.external_veth_ns:
                fam = str(addr_version(router.external_ip))
                veths.append((router.external_veth_root, router.external_veth_ns, router.netns))
                routes.append((host_cidr(router.external_ip), router.external_veth_root, "", fam))
                routes.append(("default", router.external_veth_ns, router.netns, fam))
                mark = marked(router.external_ip, router.id)
                if router.snat:
                    for iface in router.interfaces:
                        rules.append(
                            (
                                router.netns,
                                "nat",
                                "POSTROUTING",
                                finish_spec(
                                    ["-s", iface.cidr, "-o", router.external_veth_ns],
                                    mark,
                                    ["-j", "SNAT", "--to-source", router.external_ip],
                                ),
                            )
                        )
                for rule in router.dnat:
                    routes.append((host_cidr(rule.floating), router.external_veth_root, "", fam))
                    rules.append(
                        (
                            router.netns,
                            "nat",
                            "PREROUTING",
                            finish_spec(
                                ["-d", rule.floating],
                                mark,
                                ["-j", "DNAT", "--to-destination", rule.fixed],
                            ),
                        )
                    )
    return Desired(fdb, routes, veths, rules, meta, addrs)


def ensure_link(runner: Runner, created: set[str], name: str, argv: list[str]) -> bool:
    """Create a link when it is absent. Returns True when this call created it."""
    if runner.link_exists(name):
        return False
    if not runner.mutate_create(argv):
        return False
    created.add(name)
    return True


def ensure_netns(runner: Runner, known: set[str], created: set[str], name: str) -> None:
    if name in known:
        return
    if runner.mutate_create([runner.ip, "netns", "add", name]):
        created.add(name)
    known.add(name)


def ensure_provider(runner: Runner, created: set[str], net: Network, prov: Provider) -> None:
    """Attach physnet. Record only a VLAN link this call created. Never delete physnet."""
    if prov.vlan is None:
        runner.mutate([runner.ip, "link", "set", prov.physnet, "master", net.bridge])
        runner.mutate([runner.ip, "link", "set", prov.physnet, "up"])
        return
    link = prov.link or ""
    if not VLAN_LINK_RE.fullmatch(link):
        die(f"provider link {link} is not allowed")
    ensure_link(
        runner,
        created,
        link,
        [
            runner.ip,
            "link",
            "add",
            "link",
            prov.physnet,
            "name",
            link,
            "type",
            "vlan",
            "id",
            str(prov.vlan),
        ],
    )
    runner.mutate([runner.ip, "link", "set", link, "master", net.bridge])
    runner.mutate([runner.ip, "link", "set", link, "mtu", str(net.mtu), "up"])


def ensure_network(runner: Runner, created: set[str], net: Network) -> None:
    ensure_link(
        runner,
        created,
        net.bridge,
        [runner.ip, "link", "add", net.bridge, "type", "bridge"],
    )
    runner.mutate([runner.ip, "link", "set", net.bridge, "mtu", str(net.mtu), "up"])
    if net.provider is not None:
        ensure_provider(runner, created, net, net.provider)
        return
    # nolearning is set at create. Changing type on an existing device is not portable.
    ensure_link(
        runner,
        created,
        net.vxlan,
        [
            runner.ip,
            "link",
            "add",
            net.vxlan,
            "type",
            "vxlan",
            "id",
            str(net.vni),
            "local",
            net.local,
            "dstport",
            VXLAN_PORT,
            "nolearning",
        ],
    )
    runner.mutate([runner.ip, "link", "set", net.vxlan, "master", net.bridge])
    runner.mutate([runner.ip, "link", "set", net.vxlan, "mtu", str(net.mtu), "up"])


def ensure_fdb(runner: Runner, fdb: list[tuple[str, str, str]]) -> None:
    for dev, mac, dst in fdb:
        runner.mutate([runner.bridge, "fdb", "replace", mac, "dev", dev, "dst", dst])


def move_into_netns(runner: Runner, netns: str, name: str, just_created: bool) -> None:
    if just_created or runner.link_exists(name):
        runner.mutate([runner.ip, "link", "set", name, "netns", netns])


def ensure_veth_pair(
    runner: Runner,
    created: set[str],
    root: str,
    peer: str,
    netns: str,
    add: list[str],
) -> bool:
    root_in_root = runner.link_exists(root)
    peer_in_root = runner.link_exists(peer)
    peer_in_ns = runner.link_in_netns(netns, peer) if not peer_in_root else False
    just_created = False
    if not root_in_root and not peer_in_root and not peer_in_ns:
        if runner.mutate_create(add):
            created.add(root)
            created.add(peer)
            just_created = True
    return just_created


def ensure_iface(
    runner: Runner,
    plan: Plan,
    created: set[str],
    router: Router,
    iface: Iface,
) -> None:
    net = plan.by_id(iface.network_id)
    just_created = ensure_veth_pair(
        runner,
        created,
        iface.veth_bridge,
        iface.veth_ns,
        router.netns,
        [
            runner.ip,
            "link",
            "add",
            iface.veth_bridge,
            "type",
            "veth",
            "peer",
            "name",
            iface.veth_ns,
        ],
    )
    # Gateway stays on the namespace end. The root bridge must not own it.
    runner.mutate([runner.ip, "link", "set", iface.veth_bridge, "master", net.bridge])
    runner.mutate([runner.ip, "link", "set", iface.veth_bridge, "up"])
    move_into_netns(runner, router.netns, iface.veth_ns, just_created)
    fam = family_opt(addr_version(iface.gateway))
    runner.mutate(
        [
            runner.ip,
            "netns",
            "exec",
            router.netns,
            runner.ip,
            fam,
            "addr",
            "replace",
            f"{iface.gateway}/{iface.prefix}",
            "dev",
            iface.veth_ns,
        ]
    )
    runner.mutate(
        [runner.ip, "netns", "exec", router.netns, runner.ip, "link", "set", iface.veth_ns, "up"]
    )


def ensure_external(runner: Runner, created: set[str], router: Router) -> None:
    if not router.external_ip or not router.external_veth_root or not router.external_veth_ns:
        return
    root = router.external_veth_root
    peer = router.external_veth_ns
    just_created = ensure_veth_pair(
        runner,
        created,
        root,
        peer,
        router.netns,
        [runner.ip, "link", "add", root, "type", "veth", "peer", "name", peer],
    )
    runner.mutate([runner.ip, "link", "set", root, "up"])
    move_into_netns(runner, router.netns, peer, just_created)
    runner.mutate(
        [runner.ip, "netns", "exec", router.netns, runner.ip, "link", "set", peer, "up"]
    )
    fam = family_opt(addr_version(router.external_ip))
    runner.mutate(
        [
            runner.ip,
            "netns",
            "exec",
            router.netns,
            runner.ip,
            fam,
            "addr",
            "replace",
            host_cidr(router.external_ip),
            "dev",
            peer,
        ]
    )
    runner.mutate(
        [
            runner.ip,
            "netns",
            "exec",
            router.netns,
            runner.ip,
            fam,
            "route",
            "replace",
            "default",
            "dev",
            peer,
        ]
    )
    runner.mutate([runner.ip, fam, "route", "replace", host_cidr(router.external_ip), "dev", root])
    families = {addr_version(router.external_ip)}
    families.update(addr_version(iface.gateway) for iface in router.interfaces)
    families.update(addr_version(rule.floating) for rule in router.dnat)
    for version in sorted(families):
        _forward(runner, router.netns, version)
    for rule in router.dnat:
        rule_fam = family_opt(addr_version(rule.floating))
        runner.mutate(
            [
                runner.ip,
                "netns",
                "exec",
                router.netns,
                runner.ip,
                rule_fam,
                "addr",
                "replace",
                host_cidr(rule.floating),
                "dev",
                peer,
            ]
        )
        runner.mutate(
            [runner.ip, rule_fam, "route", "replace", host_cidr(rule.floating), "dev", root]
        )


def _forward(runner: Runner, netns: str, version: int) -> None:
    if version == 6:
        key = "net.ipv6.conf.all.forwarding=1"
    else:
        key = "net.ipv4.ip_forward=1"
    runner.mutate([runner.ip, "netns", "exec", netns, runner.sysctl, "-w", key])


def ensure_floating(
    runner: Runner,
    known_ns: set[str],
    created: set[str],
    net: Network,
    entry: Floating,
) -> None:
    ensure_netns(runner, known_ns, created, entry.netns)
    runner.mutate(
        [runner.ip, "netns", "exec", entry.netns, runner.ip, "link", "set", "lo", "up"]
    )
    just_created = ensure_veth_pair(
        runner,
        created,
        entry.veth_bridge,
        entry.veth_ns,
        entry.netns,
        [
            runner.ip,
            "link",
            "add",
            entry.veth_bridge,
            "type",
            "veth",
            "peer",
            "name",
            entry.veth_ns,
        ],
    )
    runner.mutate([runner.ip, "link", "set", entry.veth_bridge, "master", net.bridge])
    runner.mutate([runner.ip, "link", "set", entry.veth_bridge, "up"])
    move_into_netns(runner, entry.netns, entry.veth_ns, just_created)
    fam = family_opt(addr_version(entry.floating))
    runner.mutate(
        [
            runner.ip,
            "netns",
            "exec",
            entry.netns,
            runner.ip,
            fam,
            "addr",
            "replace",
            host_cidr(entry.floating),
            "dev",
            entry.veth_ns,
        ]
    )
    runner.mutate(
        [runner.ip, "netns", "exec", entry.netns, runner.ip, "link", "set", entry.veth_ns, "up"]
    )
    _forward(runner, entry.netns, addr_version(entry.floating))


def ensure_router(runner: Runner, plan: Plan, known_ns: set[str], created: set[str], router: Router) -> None:
    ensure_netns(runner, known_ns, created, router.netns)
    runner.mutate([runner.ip, "netns", "exec", router.netns, runner.ip, "link", "set", "lo", "up"])
    for iface in router.interfaces:
        ensure_iface(runner, plan, created, router, iface)
    ensure_external(runner, created, router)
    # external_dev is intentionally unused here.


def ensure_metadata(runner: Runner, bridges: list[str]) -> None:
    for bridge in bridges:
        if not BRIDGE_RE.fullmatch(bridge):
            die(f"metadata bridge {bridge} is not allowed")
        runner.mutate(
            [runner.ip, "-4", "addr", "replace", f"{METADATA_ADDR}/32", "dev", bridge]
        )
        runner.mutate([runner.sysctl, "-w", f"net.ipv4.conf.{bridge}.route_localnet=1"])


def ensure_sg_chain(runner: Runner, v6: bool) -> None:
    tool = runner.rule_tool(v6)
    rc, _, _ = runner.probe([tool, "-t", "filter", "-S", SG_CHAIN])
    if rc != 0:
        runner.mutate_create([tool, "-t", "filter", "-N", SG_CHAIN])
    jump = ("-j", SG_CHAIN)
    present = runner.probe(runner.rule_argv("", "filter", "-C", "FORWARD", jump, v6))[0] == 0
    if not present:
        runner.mutate(runner.rule_argv("", "filter", "-A", "FORWARD", jump, v6))


def sync_rules(
    runner: Runner,
    previous: list[tuple[str, str, str, tuple[str, ...]]],
    desired: list[tuple[str, str, str, tuple[str, ...]]],
) -> None:
    if any(
        chain == SG_CHAIN and netns == "" and not spec_v6(spec)
        for netns, _table, chain, spec in desired
    ):
        ensure_sg_chain(runner, False)
    if any(
        chain == SG_CHAIN and netns == "" and spec_v6(spec)
        for netns, _table, chain, spec in desired
    ):
        ensure_sg_chain(runner, True)
    if previous == desired:
        for netns, table, chain, spec in desired:
            if not runner.rule_present(netns, table, chain, spec):
                runner.mutate(runner.rule_argv(netns, table, "-A", chain, spec))
        return
    # Rebuild when the set changes so a new ACCEPT cannot land behind DROP.
    # A missing rule or a missing namespace is already the desired delete.
    for netns, table, chain, spec in previous:
        if runner.rule_present(netns, table, chain, spec):
            _delete_rule(runner, netns, table, chain, spec)
    for netns, table, chain, spec in desired:
        if runner.rule_present(netns, table, chain, spec):
            _delete_rule(runner, netns, table, chain, spec)
        runner.mutate(runner.rule_argv(netns, table, "-A", chain, spec))


def _delete_rule(
    runner: Runner, netns: str, table: str, chain: str, spec: tuple[str, ...]
) -> None:
    runner.mutate_missing(
        runner.rule_argv(netns, table, "-D", chain, spec),
        extra_ok=("bad rule", "does a matching rule exist", "cannot open"),
    )


def retire_routes(
    runner: Runner,
    previous: list[tuple[str, str, str, str]],
    desired: set[tuple[str, str, str, str]],
) -> None:
    for dest, dev, netns, fam in previous:
        if (dest, dev, netns, fam) in desired:
            continue
        if not ROUTE_DEV_RE.fullmatch(dev):
            die(f"refusing to delete route dev {dev}")
        if fam not in ("4", "6"):
            die(f"refusing to delete route family {fam}")
        cmd = [runner.ip, family_opt(int(fam)), "route", "del", dest, "dev", dev]
        if netns:
            if not ANY_NS_RE.fullmatch(netns):
                die(f"refusing to delete route in {netns}")
            cmd = [runner.ip, "netns", "exec", netns, *cmd]
        runner.mutate_missing(cmd)


def retire_addrs(
    runner: Runner,
    previous: list[tuple[str, str, str]],
    desired: set[tuple[str, str, str]],
) -> None:
    for addr, dev, netns in previous:
        if (addr, dev, netns) in desired:
            continue
        if not ADDR_DEV_RE.fullmatch(dev) or not FLOAT_NS_RE.fullmatch(netns):
            die(f"refusing to delete address on {dev}")
        fam = family_opt(addr_version(addr))
        runner.mutate_missing(
            [
                runner.ip,
                "netns",
                "exec",
                netns,
                runner.ip,
                fam,
                "addr",
                "del",
                addr,
                "dev",
                dev,
            ],
            extra_ok=("cannot assign",),
        )


def retire_veths(
    runner: Runner,
    previous: list[tuple[str, str, str]],
    desired_roots: set[str],
    created: set[str],
) -> None:
    for root, peer, _netns in previous:
        if root in desired_roots:
            continue
        if not VETH_ROOT_RE.fullmatch(root) or not VETH_PEER_RE.fullmatch(peer):
            die(f"refusing to delete veth {root}")
        runner.mutate_missing([runner.ip, "link", "del", root])
        created.discard(root)
        created.discard(peer)


_OWNED_RANK = {"veth": 0, "vxlan": 1, "bridge": 2, "netns": 3}


def _refuse_delete(name: str) -> None:
    if (
        not name
        or name in ("pistack0", "wg-fleet", "simbr0")
        or name.startswith("pi")
    ):
        die(f"refusing to delete {name or 'empty name'}")


def _owned_kind(name: str) -> Optional[str]:
    if VETH_ROOT_RE.fullmatch(name):
        return "veth"
    if VXLAN_RE.fullmatch(name):
        return "vxlan"
    if BRIDGE_RE.fullmatch(name):
        return "bridge"
    if ANY_NS_RE.fullmatch(name):
        return "netns"
    return None


def planned_owned(plan: Plan) -> set[str]:
    """Bridge, VXLAN, netns, and veth names the plan still wants."""
    wanted: set[str] = set()
    for net in plan.networks:
        wanted.add(net.bridge)
        if net.vxlan:
            wanted.add(net.vxlan)
        for router in net.routers:
            wanted.add(router.netns)
            if router.external_veth_root:
                wanted.add(router.external_veth_root)
            if router.external_veth_ns:
                wanted.add(router.external_veth_ns)
            for iface in router.interfaces:
                wanted.add(iface.veth_bridge)
                wanted.add(iface.veth_ns)
        for entry in net.floating:
            wanted.add(entry.netns)
            wanted.add(entry.veth_bridge)
            wanted.add(entry.veth_ns)
    return wanted


def retire_owned(runner: Runner, created: set[str], plan: Plan) -> None:
    """Delete bridge, vxlan, netns, and leftover veth this tool created.

    retire_veths already removed pairs listed in the previous state. This
    pass removes recorded devices the new plan dropped, and a device that
    is already gone. VLAN links and physical ports are not in this set.
    """
    desired = planned_owned(plan)
    stale = [name for name in created if name not in desired]
    for name in stale:
        _refuse_delete(name)
    ranked = sorted(
        (name for name in stale if _owned_kind(name) is not None),
        key=lambda name: (_OWNED_RANK[_owned_kind(name) or ""], name),
    )
    for name in ranked:
        kind = _owned_kind(name)
        if kind == "netns":
            runner.mutate_missing([runner.ip, "netns", "del", name])
        else:
            runner.mutate_missing([runner.ip, "link", "del", name])
        created.discard(name)
        if kind == "veth":
            peer = {"a": "b", "c": "d", "e": "f"}.get(name[0])
            if peer is not None:
                created.discard(peer + name[1:])


def retire_fdb(
    runner: Runner,
    previous: list[tuple[str, str, str]],
    desired: set[tuple[str, str, str]],
) -> None:
    for dev, mac, dst in previous:
        if (dev, mac, dst) in desired:
            continue
        if not VXLAN_RE.fullmatch(dev) or dst == "0.0.0.0":
            die(f"refusing to delete fdb on {dev}")
        runner.mutate_missing([runner.bridge, "fdb", "del", mac, "dev", dev, "dst", dst])


def owned_veths(
    created: set[str],
    previous: list[tuple[str, str, str]],
    desired: list[tuple[str, str, str]],
) -> list[tuple[str, str, str]]:
    """Veths this tool created that the plan still wants."""
    previous_roots = {root for root, _peer, _netns in previous}
    out = []
    seen: set[str] = set()
    for root, peer, netns in desired:
        if root in seen:
            continue
        if root in created or peer in created or root in previous_roots:
            seen.add(root)
            out.append((root, peer, netns))
    return out


def dnsmasq_bin() -> str:
    override = os.environ.get("PISTACK_OVERLAY_DNSMASQ")
    if override:
        if not os.path.isabs(override):
            die("PISTACK_OVERLAY_DNSMASQ must be an absolute path")
        if not os.path.isfile(override):
            fail(f"cannot run {override}: missing dnsmasq")
        return override
    found = shutil.which("dnsmasq")
    if not found:
        fail("cannot run dnsmasq: not found")
    return found


def dhcp_paths(netns: str, veth: str) -> tuple[str, str, str]:
    base = os.path.join(state_dir(), f"dhcp-{netns}-{veth}")
    return base + ".conf", base + ".pid", base + ".leases"


def render_dhcp(iface: Iface, pid_path: str, lease_path: str) -> list[str]:
    """Directives for one namespace veth. Unknown clients get no lease."""
    lines = [
        f"interface={iface.veth_ns}",
        "bind-interfaces",
        "port=0",
    ]
    gateway = ipaddress.ip_address(iface.gateway)
    if gateway.version == 4:
        mask = str(ipaddress.IPv4Network(f"0.0.0.0/{iface.prefix}").netmask)
        lines.append(f"dhcp-range={iface.gateway},static,{mask}")
        lines.append(f"dhcp-option=option:router,{iface.gateway}")
    saw_v6 = False
    for lease in iface.dhcp:
        addr = ipaddress.ip_address(lease.address)
        if addr.version == 6:
            saw_v6 = True
            lines.append(f"dhcp-host={lease.mac},[{lease.address}]")
        else:
            lines.append(f"dhcp-host={lease.mac},{lease.address}")
        v4_dns = [ip for ip in lease.dns if addr_version(ip) == 4]
        v6_dns = [ip for ip in lease.dns if addr_version(ip) == 6]
        if v4_dns:
            lines.append("dhcp-option=option:dns-server," + ",".join(v4_dns))
        if v6_dns:
            braced = ",".join(f"[{ip}]" for ip in v6_dns)
            lines.append(f"dhcp-option=option6:dns-server,{braced}")
        for route in lease.routes:
            lines.append(
                "dhcp-option=option:classless-static-route,"
                f"{route.destination},{route.nexthop}"
            )
    if saw_v6:
        lines.append("enable-ra")
    lines.append(f"pid-file={pid_path}")
    lines.append(f"dhcp-leasefile={lease_path}")
    deduped: list[str] = []
    seen: set[str] = set()
    for line in lines:
        if line in seen:
            continue
        seen.add(line)
        deduped.append(line)
    return deduped


def read_pid(path: str) -> Optional[int]:
    try:
        with open(path, encoding="utf-8") as fh:
            text = fh.read().strip()
        pid = int(text)
    except (OSError, ValueError):
        return None
    if pid <= 1:
        return None
    return pid


def pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True


def pid_is_ours(pid: int, pid_path: str) -> bool:
    proc = f"/proc/{pid}/cmdline"
    try:
        with open(proc, "rb") as fh:
            blob = fh.read().replace(b"\x00", b" ")
    except OSError:
        return True
    text = blob.decode("utf-8", "replace")
    return pid_path in text


def stop_dhcp(runner: Runner, pid_path: str) -> None:
    """Stop the pid recorded for a lease file this tool started."""
    pid = read_pid(pid_path)
    if pid is None:
        return
    if not pid_alive(pid) or not pid_is_ours(pid, pid_path):
        if not runner.dry:
            try:
                os.remove(pid_path)
            except OSError:
                pass
        return
    if runner.dry:
        print(f"kill {pid}")
        return
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError:
        pass
    for _ in range(20):
        if not pid_alive(pid):
            break
        time.sleep(0.05)
    try:
        os.remove(pid_path)
    except OSError:
        pass


def write_dhcp_conf(path: str, text: str) -> None:
    os.makedirs(state_dir(), mode=0o755, exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        fh.write(text)
    os.chmod(tmp, 0o644)
    os.replace(tmp, path)


def dhcp_argv(netns: str, lines: list[str]) -> list[str]:
    # --conf-file=/dev/null ignores /etc/dnsmasq.conf. The same lines are the
    # rendered config; passing them twice would trip duplicate dhcp-host.
    argv = [
        resolve_tool("ip"),
        "netns",
        "exec",
        netns,
        dnsmasq_bin(),
        "--conf-file=/dev/null",
    ]
    argv.extend(f"--{line}" for line in lines)
    return argv


def read_text(path: str) -> Optional[str]:
    try:
        with open(path, encoding="utf-8") as fh:
            return fh.read()
    except OSError:
        return None


def desired_dhcp(plan: Plan) -> list[tuple[str, Iface]]:
    found: list[tuple[str, Iface]] = []
    seen: set[str] = set()
    for net in plan.networks:
        for router in net.routers:
            if router.id in seen:
                continue
            seen.add(router.id)
            for iface in router.interfaces:
                if iface.dhcp:
                    found.append((router.netns, iface))
    return found


def sync_dhcp(runner: Runner, plan: Plan) -> None:
    wanted = desired_dhcp(plan)
    if wanted:
        dnsmasq_bin()
    keep = {(netns, iface.veth_ns) for netns, iface in wanted}
    directory = state_dir()
    if os.path.isdir(directory):
        for name in os.listdir(directory):
            match = DHCP_FILE_RE.fullmatch(name)
            if match is None:
                continue
            key = (match.group(1), match.group(2))
            if key in keep:
                continue
            conf, pid, _lease = dhcp_paths(*key)
            stop_dhcp(runner, pid)
            if not runner.dry:
                for path in (conf, pid):
                    try:
                        os.remove(path)
                    except OSError:
                        pass
    for netns, iface in wanted:
        conf, pid, lease = dhcp_paths(netns, iface.veth_ns)
        lines = render_dhcp(iface, pid, lease)
        text = "\n".join(lines) + "\n"
        current = read_text(conf)
        running = read_pid(pid)
        if current == text and running is not None and pid_alive(running):
            continue
        stop_dhcp(runner, pid)
        if not runner.dry:
            write_dhcp_conf(conf, text)
        runner.mutate(dhcp_argv(netns, lines))


def read_plan(path: str) -> str:
    if path == "-":
        return sys.stdin.read()
    try:
        with open(path, encoding="utf-8") as fh:
            return fh.read()
    except OSError as exc:
        die(f"cannot read plan: {exc}")


def cmd_apply(path: str, dry: bool) -> None:
    apply_text(read_plan(path), dry)


def converge(plan: Plan, dry: bool) -> None:
    runner = Runner(dry)
    state = load_state()
    comment_v6 = runner.comments_for(True) if plan_uses_v6(plan) else False
    desired = build_desired(plan, runner.comments(), comment_v6)
    created = set(state.created)
    known_ns = runner.netns_names()
    for net in plan.networks:
        ensure_network(runner, created, net)
    ensure_fdb(runner, desired.fdb)
    seen_routers: set[str] = set()
    for net in plan.networks:
        for router in net.routers:
            if router.id in seen_routers:
                continue
            seen_routers.add(router.id)
            ensure_router(runner, plan, known_ns, created, router)
    seen_float: set[str] = set()
    for net in plan.networks:
        for entry in net.floating:
            if entry.id in seen_float:
                continue
            seen_float.add(entry.id)
            ensure_floating(runner, known_ns, created, net, entry)
    ensure_metadata(runner, desired.metadata_bridges)
    sync_dhcp(runner, plan)
    sync_rules(runner, state.rules, desired.rules)
    retire_addrs(runner, state.addrs, set(desired.addrs))
    retire_routes(runner, state.routes, set(desired.routes))
    desired_roots = {root for root, _peer, _netns in desired.veths}
    retire_veths(runner, state.veths, desired_roots, created)
    retire_fdb(runner, state.fdb, set(desired.fdb))
    retire_owned(runner, created, plan)
    if dry:
        return
    save_state(
        State(
            created=created,
            fdb=list(desired.fdb),
            routes=list(desired.routes),
            veths=owned_veths(created, state.veths, desired.veths),
            rules=list(desired.rules),
            addrs=list(desired.addrs),
        )
    )


def apply_text(text: str, dry: bool) -> None:
    try:
        data = json.loads(text)
    except json.JSONDecodeError as exc:
        die(f"plan is not json: {exc}")
    converge(parse_plan(data), dry)


def _refuses(fn, what: str) -> None:
    held = sys.stderr
    buf = io.StringIO()
    sys.stderr = buf
    try:
        fn()
    except SystemExit as exc:
        sys.stderr = held
        if exc.code == 2:
            return
        fail(f"self-test: {what} exited {exc.code}: {buf.getvalue().strip()}")
    except Exception as exc:
        sys.stderr = held
        fail(f"self-test: {what} raised {exc}")
    else:
        sys.stderr = held
        fail(f"self-test: {what} was accepted")
    finally:
        sys.stderr = held


def _assert(cond: bool, what: str) -> None:
    if not cond:
        fail(f"self-test: {what}")


def sample_plan(with_peer: bool) -> dict:
    """Two tenants, one CIDR, distinct VNIs and namespaces."""
    peers = ["10.0.0.2"] if with_peer else []

    def network(
        net_id: str,
        vni: int,
        bridge: str,
        vxlan: str,
        router_id: str,
        netns: str,
        veth_br: str,
        veth_ns: str,
        ext_root: str,
        ext_ns: str,
        external_ip: str,
        ports: list,
        dnat: list,
    ) -> dict:
        return {
            "id": net_id,
            "vni": vni,
            "mtu": 1450,
            "bridge": bridge,
            "vxlan": vxlan,
            "local": "10.0.0.1",
            "peers": list(peers),
            "ports": ports,
            "routers": [
                {
                    "id": router_id,
                    "netns": netns,
                    "interfaces": [
                        {
                            "network_id": net_id,
                            "veth_bridge": veth_br,
                            "veth_ns": veth_ns,
                            "gateway": "10.90.0.1",
                            "prefix": 24,
                            "cidr": "10.90.0.0/24",
                            "dhcp": [
                                {
                                    "mac": "02:70:69:1f:f3:21",
                                    "address": "10.90.0.10",
                                    "dns": ["1.1.1.1"],
                                    "routes": [
                                        {
                                            "destination": "192.168.50.0/24",
                                            "nexthop": "10.90.0.1",
                                        }
                                    ],
                                }
                            ],
                        }
                    ],
                    "external_veth_root": ext_root,
                    "external_veth_ns": ext_ns,
                    "external_ip": external_ip,
                    "external_dev": "wg-fleet",
                    "snat": True,
                    "dnat": dnat,
                    "metadata": True,
                }
            ],
        }

    remote = {
        "id": "port-1",
        "mac": "02:70:69:1f:f3:20",
        "address": "10.90.0.2",
        "underlay": "10.0.0.2",
        "ingress": [{"proto": "tcp", "port_min": 22, "port_max": 22, "cidr": "10.0.0.0/24"}],
        "egress": [],
    }
    local = {
        "id": "port-local",
        "mac": "02:70:69:aa:00:11",
        "address": "10.90.0.3",
        "underlay": "10.0.0.1",
        "ingress": [],
        "egress": [],
    }
    v6_port = {
        "id": "port-v6",
        "mac": "02:70:69:aa:00:66",
        "address": "2001:db8::10",
        "underlay": "10.0.0.3",
        "ingress": [{"proto": "icmp", "port_min": 8, "port_max": 8}],
        "egress": [],
    }
    return {
        "version": 1,
        "host": "10.0.0.1",
        "networks": [
            network(
                "net-01a10318-a507-74fd-a86e-5acbda974722",
                7,
                "po01a10318a507",
                "vx01a10318a507",
                "rtr-0123456789abcdef0123456789abcdef",
                "ns0123456789ab",
                "a01a103012345",
                "b01a103012345",
                "c0123456789ab",
                "d0123456789ab",
                "10.66.66.84",
                [remote, local, v6_port],
                [{"id": "fip-1", "floating": "10.66.66.85", "fixed": "10.90.0.2"}],
            ),
            network(
                "net-01b20318-b607-74fd-a86e-5acbda974733",
                8,
                "po01b20318b607",
                "vx01b20318b607",
                "rtr-fedcba9876543210fedcba9876543210",
                "nsfedcba987654",
                "a01b203065432",
                "b01b203065432",
                "cfedcba987654",
                "dfedcba987654",
                "10.66.66.86",
                [],
                [],
            ),
            {
                "id": "net-provider-0001",
                "vni": 9,
                "mtu": 1500,
                "bridge": "po000000000009",
                "vxlan": "vx000000000009",
                "local": "10.0.0.1",
                "peers": [],
                "ports": [],
                "routers": [],
                "provider": {
                    "physnet": "eno1",
                    "vlan": 20,
                    "link": "pv0123456789ab",
                },
            },
        ],
    }


def _check_refusals() -> None:
    _refuses(lambda: parse_plan({"version": 2, "host": "10.0.0.1", "networks": []}), "version 2")
    base = sample_plan(True)
    bad_bridge = json.loads(json.dumps(base))
    bad_bridge["networks"][0]["bridge"] = "pistack0"
    _refuses(lambda: parse_plan(bad_bridge), "bridge pistack0")
    bad_pi = json.loads(json.dumps(base))
    bad_pi["networks"][0]["bridge"] = "pi0123456789ab"
    _refuses(lambda: parse_plan(bad_pi), "bridge pi hex")
    bad_peer = json.loads(json.dumps(base))
    bad_peer["networks"][0]["peers"] = ["0.0.0.0"]
    _refuses(lambda: parse_plan(bad_peer), "peer 0.0.0.0")
    bad_local = json.loads(json.dumps(base))
    bad_local["networks"][0]["peers"] = ["10.0.0.1"]
    _refuses(lambda: parse_plan(bad_local), "peer is local")
    bad_mcast = json.loads(json.dumps(base))
    bad_mcast["host"] = "224.0.0.1"
    _refuses(lambda: parse_plan(bad_mcast), "multicast host")
    bad_vni = json.loads(json.dumps(base))
    bad_vni["networks"][0]["vni"] = 0
    _refuses(lambda: parse_plan(bad_vni), "vni 0")
    bad_mtu = json.loads(json.dumps(base))
    bad_mtu["networks"][0]["mtu"] = 1501
    _refuses(lambda: parse_plan(bad_mtu), "mtu 1501")
    bad_prefix = json.loads(json.dumps(base))
    bad_prefix["networks"][0]["routers"][0]["interfaces"][0]["prefix"] = 31
    _refuses(lambda: parse_plan(bad_prefix), "prefix 31")
    bad_proto = json.loads(json.dumps(base))
    bad_proto["networks"][0]["ports"][0]["ingress"][0]["proto"] = "igmp"
    _refuses(lambda: parse_plan(bad_proto), "proto igmp")
    bad_cidr = json.loads(json.dumps(base))
    bad_cidr["networks"][0]["ports"][0]["ingress"][0]["cidr"] = "not-a-cidr"
    _refuses(lambda: parse_plan(bad_cidr), "bad cidr")
    bad_phys = json.loads(json.dumps(base))
    bad_phys["networks"][0]["provider"] = {
        "physnet": "pi0123456789ab",
        "vlan": 20,
        "link": "pv0123456789ab",
    }
    _refuses(lambda: parse_plan(bad_phys), "physnet pi")
    bad_wg = json.loads(json.dumps(base))
    bad_wg["networks"][0]["provider"] = {
        "physnet": "wg-fleet",
        "vlan": None,
    }
    _refuses(lambda: parse_plan(bad_wg), "physnet wg-fleet")
    bad_space = json.loads(json.dumps(base))
    bad_space["networks"][0]["provider"] = {"physnet": "eno 1", "vlan": None}
    _refuses(lambda: parse_plan(bad_space), "physnet whitespace")
    bad_link = json.loads(json.dumps(base))
    bad_link["networks"][0]["provider"] = {
        "physnet": "eno1",
        "vlan": 20,
        "link": "vx0123456789ab",
    }
    _refuses(lambda: parse_plan(bad_link), "provider link prefix")
    bad_vlan = json.loads(json.dumps(base))
    bad_vlan["networks"][0]["provider"] = {"physnet": "eno1", "vlan": 0, "link": "pv0123456789ab"}
    _refuses(lambda: parse_plan(bad_vlan), "vlan 0")
    collide = json.loads(json.dumps(base))
    collide["networks"][0]["floating"] = [
        {
            "id": "fip-collide",
            "floating": "10.66.66.84",
            "fixed": "10.90.0.50",
            "netns": "nf0123456789ab",
            "veth_bridge": "e0123456789ab",
            "veth_ns": "f0123456789ab",
        }
    ]
    _refuses(lambda: parse_plan(collide), "floating collides with external_ip")
    bare_v6 = json.loads(json.dumps(base))
    bare_v6["networks"][0]["peers"] = ["2001:db8::2"]
    _refuses(lambda: parse_plan(bare_v6), "ipv6 peer")
    public_host = {"version": 1, "host": "8.8.8.8", "networks": []}
    _refuses(lambda: parse_plan(public_host), "public underlay")
    loop_host = {"version": 1, "host": "127.0.0.1", "networks": []}
    _refuses(lambda: parse_plan(loop_host), "loopback underlay")
    for bad_host in ("100.63.255.255", "100.128.0.1", "172.32.0.1", "192.169.0.1", "11.0.0.1"):
        _refuses(
            lambda host=bad_host: parse_plan({"version": 1, "host": host, "networks": []}),
            f"underlay {bad_host}",
        )
    _refuses(lambda: parse_plan({"version": 1, "host": "", "networks": []}), "empty underlay")
    for good_host in ("10.0.0.1", "172.16.0.1", "172.31.255.254", "192.168.1.1", "100.64.0.1", "100.127.255.254"):
        parse_plan({"version": 1, "host": good_host, "networks": []})
    bad_peer = json.loads(json.dumps(base))
    bad_peer["networks"][0]["peers"] = ["8.8.8.8"]
    _refuses(lambda: parse_plan(bad_peer), "public peer")
    bad_port_ul = json.loads(json.dumps(base))
    bad_port_ul["networks"][0]["ports"][0]["underlay"] = "1.2.3.4"
    _refuses(lambda: parse_plan(bad_port_ul), "public port underlay")
    bad_sim = json.loads(json.dumps(base))
    bad_sim["networks"][0]["provider"] = {"physnet": "simbr0", "vlan": None}
    _refuses(lambda: parse_plan(bad_sim), "physnet simbr0")
    bad_empty_phys = json.loads(json.dumps(base))
    bad_empty_phys["networks"][0]["provider"] = {"physnet": "", "vlan": None}
    _refuses(lambda: parse_plan(bad_empty_phys), "empty physnet")
    runner = Runner(True)
    for bad in ("pistack0", "wg-fleet", "simbr0", "", "pi0123456789ab", "pibr0"):
        _refuses(lambda tok=bad: runner.guard(["ip", "link", "del", tok]), f"guard {bad or 'empty'}")
    runner.guard(["iptables", "-m", "comment", "--comment", f"{COMMENT_PREFIX}port-1"])
    public_v6 = json.loads(json.dumps(v6_snat_plan()))
    public_v6["networks"][0]["routers"][0]["external_ip"] = "2001:db8::9"
    _refuses(lambda: parse_plan(public_v6), "public ipv6 external")
    parse_plan(v6_snat_plan())
    parse_plan(base)


def _install_fakes(bindir: str, log_path: str) -> None:
    os.makedirs(bindir, mode=0o755)
    for name in ("ip", "bridge", "iptables", "sysctl", "dnsmasq", "ip6tables"):
        path = os.path.join(bindir, name)
        lines = [
            "#!/usr/bin/env python3",
            "import json",
            "import sys",
            f"with open({log_path!r}, 'a', encoding='utf-8') as fh:",
            '    fh.write(json.dumps(sys.argv) + "\\n")',
            f'if {name!r} == "ip" and "link" in sys.argv and "show" in sys.argv:',
            "    sys.exit(1)",
            "sys.exit(0)",
            "",
        ]
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("\n".join(lines))
        os.chmod(path, 0o755)


def _load_log(path: str) -> list[list[str]]:
    entries: list[list[str]] = []
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            item = json.loads(line)
            if not isinstance(item, list) or not all(isinstance(tok, str) for tok in item):
                fail("self-test: fake log is not argv json")
            entries.append(item)
    return entries


def _has(argv: list[str], *tokens: str) -> bool:
    return all(tok in argv for tok in tokens)


def _check_log(first: list[list[str]], second: list[list[str]]) -> None:
    entries = first + second
    vxlan = [
        argv
        for argv in entries
        if _has(argv, "vxlan", "nolearning", "dstport", "4789", "local", "10.0.0.1", "id")
    ]
    _assert(any(_has(argv, "7") for argv in vxlan), "vxlan create for vni 7")
    _assert(any(_has(argv, "8") for argv in vxlan), "vxlan create for vni 8")
    _assert(any(_has(argv, "fdb", "dst", "10.0.0.2") for argv in entries), "fdb dst 10.0.0.2")
    _assert(
        any(_has(argv, "fdb", "02:70:69:1f:f3:20", "dst", "10.0.0.2") for argv in entries),
        "remote mac fdb",
    )
    _assert(
        not any("02:70:69:aa:00:11" in argv for argv in entries),
        "local port has no fdb",
    )
    for argv in entries:
        for tok in argv:
            _assert("pistack0" not in tok, f"pistack0 in {argv}")
            _assert("wg-fleet" not in tok, f"wg-fleet in {argv}")
            _assert("PISTACK-SG" not in tok, f"PISTACK-SG in {argv}")
            _assert("all.route_localnet" not in tok, f"global route_localnet in {argv}")
            _assert(
                not re.search(r"(^|[^0-9])0\.0\.0\.0([^0-9]|$)", tok),
                f"0.0.0.0 in {argv}",
            )
            if "/" in tok or tok.startswith("-") or "=" in tok:
                continue
            if tok.startswith(COMMENT_PREFIX) or tok in ("ip", "bridge", "iptables", "sysctl"):
                continue
            _assert(not tok.startswith("pi"), f"device {tok} starts with pi")
            if re.fullmatch(r"po[0-9a-f]+", tok):
                _assert(BRIDGE_RE.fullmatch(tok) is not None, f"bridge {tok} is not po + 12 hex")
    _assert(any("po01a10318a507" in argv for argv in entries), "bridge po01a10318a507")
    _assert(any("po01b20318b607" in argv for argv in entries), "bridge po01b20318b607")
    for name in ("ns0123456789ab", "nsfedcba987654"):
        _assert(
            any(_has(argv, "netns", "add", name) for argv in entries),
            f"netns add {name}",
        )
        _assert(
            any(_has(argv, "netns", "exec", name, "10.90.0.1/24") for argv in entries),
            f"gateway inside netns {name}",
        )
    _assert(
        all("exec" in argv for argv in entries if "10.90.0.1/24" in argv),
        "gateway address is only inside ip netns exec",
    )
    _assert(
        any(_has(argv, "169.254.169.254/32", "po01a10318a507") for argv in entries),
        "metadata address on overlay bridge",
    )
    _assert(
        any("net.ipv4.conf.po01a10318a507.route_localnet=1" in argv for argv in entries),
        "per-bridge route_localnet",
    )
    _assert(any("127.0.0.1:18880" in argv for argv in entries), "metadata dnat")
    _assert(any("10.66.66.85" in argv for argv in entries), "dnat floating address")
    dels = [argv for argv in second if _has(argv, "fdb", "del", "10.0.0.2")]
    _assert(bool(dels), "second apply deletes fdb dst 10.0.0.2")
    _assert(any(ZERO_MAC in argv for argv in dels), "deleted fdb is the peer flood entry")
    _assert(
        any(
            _has(argv, "link", "add", "link", "eno1", "name", "pv0123456789ab", "type", "vlan", "id", "20")
            for argv in entries
        ),
        "provider vlan link",
    )
    _assert(not any("vx000000000009" in argv for argv in entries), "provider network creates no vxlan")
    _assert(not any("eno1" in argv and "vxlan" in argv for argv in entries), "physnet is not a vxlan")
    dhcp = [
        argv
        for argv in entries
        if any(tok == "dnsmasq" or tok.endswith("/dnsmasq") for tok in argv)
    ]
    _assert(dhcp, "dnsmasq invocation")
    _assert(
        any(any("dhcp-host" in tok for tok in argv) for argv in dhcp),
        "dnsmasq dhcp-host",
    )
    _assert(
        any(any("10.90.0.1,static,255.255.255.0" in tok for tok in argv) for argv in dhcp),
        "dnsmasq static range",
    )
    _assert(
        all(
            "exec" in argv and ("ns0123456789ab" in argv or "nsfedcba987654" in argv)
            for argv in dhcp
        ),
        "dnsmasq inside router netns",
    )
    _assert(
        all(not any("keep-in-foreground" in tok for tok in argv) for argv in dhcp),
        "dnsmasq is not kept in the foreground",
    )
    v6 = [argv for argv in entries if any(tok == "ip6tables" or tok.endswith("/ip6tables") for tok in argv)]
    _assert(v6, "ipv6 rules use ip6tables")
    _assert(any("2001:db8::10/128" in argv for argv in v6), "v6 host prefix")
    _assert(any("ipv6-icmp" in argv for argv in v6), "v6 icmp")
    _assert(
        not any("--dport" in argv and "ipv6-icmp" in argv for argv in v6),
        "icmp ignores ports",
    )


def _tool(argv: list[str], name: str) -> bool:
    return any(tok == name or tok.endswith("/" + name) for tok in argv)


def v6_snat_plan() -> dict:
    """One IPv6 tenant prefix SNATed to a unique-local external address."""
    return {
        "version": 1,
        "host": "10.0.0.1",
        "networks": [
            {
                "id": "net-v6",
                "vni": 11,
                "mtu": 1450,
                "bridge": "po0123456789aa",
                "vxlan": "vx0123456789aa",
                "local": "10.0.0.1",
                "peers": [],
                "ports": [],
                "routers": [
                    {
                        "id": "rtr-v6",
                        "netns": "ns0123456789ac",
                        "interfaces": [
                            {
                                "network_id": "net-v6",
                                "veth_bridge": "a0123456789ac",
                                "veth_ns": "b0123456789ac",
                                "gateway": "fd00:90::1",
                                "prefix": 64,
                                "cidr": "fd00:90::/64",
                            }
                        ],
                        "external_veth_root": "c0123456789ac",
                        "external_veth_ns": "d0123456789ac",
                        "external_ip": "fd00:66::9",
                        "external_dev": "external",
                        "snat": True,
                        "dnat": [],
                        "metadata": False,
                    }
                ],
            }
        ],
    }


def floating_plan(with_fip: bool) -> dict:
    floating: list[dict] = []
    if with_fip:
        floating = [
            {
                "id": "fip-h2",
                "floating": "203.0.113.10",
                "fixed": "10.90.0.50",
                "netns": "nf0123456789ab",
                "veth_bridge": "e0123456789ab",
                "veth_ns": "f0123456789ab",
            }
        ]
    return {
        "version": 1,
        "host": "10.0.0.2",
        "networks": [
            {
                "id": "net-ext-host-2",
                "vni": 100,
                "mtu": 1500,
                "bridge": "poaaaaaaaaaaaa",
                "vxlan": "vxaaaaaaaaaaaa",
                "local": "10.0.0.2",
                "peers": [],
                "ports": [],
                "routers": [],
                "floating": floating,
            }
        ],
    }


def _check_floating(first: list[list[str]], second: list[list[str]]) -> None:
    _assert(
        any(_has(argv, "addr", "replace", "203.0.113.10/32") for argv in first),
        "floating /32",
    )
    dnat = [
        argv
        for argv in first
        if "DNAT" in argv and "203.0.113.10/32" in argv and "10.90.0.50" in argv
    ]
    snat = [
        argv
        for argv in first
        if "SNAT" in argv and "10.90.0.50/32" in argv and "203.0.113.10" in argv
    ]
    _assert(dnat, "floating dnat pair")
    _assert(snat, "floating snat pair")
    _assert(all("nf0123456789ab" in argv for argv in dnat + snat), "nat stays in the floating netns")
    _assert(
        not any("10.90.0.0/24" in argv for argv in dnat + snat),
        "floating snat is not the subnet",
    )
    _assert(all(not _tool(argv, "ip6tables") for argv in dnat + snat), "v4 floating uses iptables")
    deleted = [
        argv
        for argv in second
        if "-D" in argv and ("203.0.113.10" in " ".join(argv) or "10.90.0.50" in " ".join(argv))
    ]
    _assert(deleted, "dropping floating deletes nat rules")
    _assert(
        not any("-A" in argv and "203.0.113.10" in " ".join(argv) for argv in second),
        "dropped floating rules are not readded",
    )
    _assert(
        any(_has(argv, "addr", "del", "203.0.113.10/32") for argv in second),
        "dropping floating deletes the address",
    )
    _assert(
        any(_has(argv, "link", "del", "e0123456789ab") for argv in second),
        "dropping floating deletes the veth",
    )
    _assert(
        not any(_has(argv, "link", "del", "poaaaaaaaaaaaa") for argv in second),
        "dropping floating keeps the bridge",
    )
    _assert(
        any(_has(argv, "netns", "del", "nf0123456789ab") for argv in second),
        "dropping floating deletes its netns",
    )
    _assert(
        not any(_has(argv, "link", "del", "vxaaaaaaaaaaaa") for argv in second),
        "dropping floating keeps the vxlan",
    )


def _check_delete_and_v6(td: str, log_path: str) -> None:
    """Dropping a record removes only its bridge, vxlan, netns, and veth.

    A second delete, and a delete of devices that are already gone, succeed.
    Private IPv6 external SNAT is an ip6tables rule in the router namespace.
    """
    os.environ["PISTACK_OVERLAY_STATE"] = os.path.join(td, "state-drop")
    with open(log_path, "w", encoding="utf-8"):
        pass
    apply_text(json.dumps(sample_plan(False)), False)
    created_log = _load_log(log_path)
    kept = sample_plan(False)
    kept["networks"][0]["routers"] = []
    apply_text(json.dumps(kept), False)
    dropped_router = _load_log(log_path)[len(created_log) :]
    _assert(
        any(_has(argv, "netns", "del", "ns0123456789ab") for argv in dropped_router),
        "dropping a router deletes its netns",
    )
    _assert(
        any(_has(argv, "link", "del", "a01a103012345") for argv in dropped_router),
        "dropping a router deletes its veth",
    )
    _assert(
        any(_has(argv, "link", "del", "c0123456789ab") for argv in dropped_router),
        "dropping a router deletes its external veth",
    )
    _assert(
        not any(_has(argv, "link", "del", "po01a10318a507") for argv in dropped_router),
        "dropping a router keeps the bridge",
    )
    _assert(
        not any(_has(argv, "link", "del", "vx01a10318a507") for argv in dropped_router),
        "dropping a router keeps the vxlan",
    )
    _assert(
        not any(_has(argv, "netns", "del", "nsfedcba987654") for argv in dropped_router),
        "the other router netns stays",
    )
    after_router = _load_log(log_path)
    empty = {"version": 1, "host": "10.0.0.1", "networks": []}
    apply_text(json.dumps(empty), False)
    dropped_nets = _load_log(log_path)[len(after_router) :]
    for name in (
        "po01a10318a507",
        "po01b20318b607",
        "po000000000009",
        "vx01a10318a507",
        "vx01b20318b607",
    ):
        _assert(any(_has(argv, "link", "del", name) for argv in dropped_nets), f"delete {name}")
    _assert(
        any(_has(argv, "netns", "del", "nsfedcba987654") for argv in dropped_nets),
        "dropping the network deletes the remaining netns",
    )
    for forbidden in ("pistack0", "wg-fleet", "simbr0", "eno1", "pv0123456789ab"):
        _assert(
            not any(forbidden in argv and "del" in argv for argv in dropped_nets),
            f"delete does not remove {forbidden}",
        )
    _assert(
        all(not any(tok.startswith("pi") and not tok.startswith(COMMENT_PREFIX) for tok in argv) for argv in dropped_nets),
        "delete commands do not name a pi device",
    )
    marked = _load_log(log_path)
    apply_text(json.dumps(empty), False)
    second = _load_log(log_path)[len(marked) :]
    _assert(not any("del" in argv for argv in second), "second delete is a no-op")

    os.environ["PISTACK_OVERLAY_STATE"] = os.path.join(td, "state-v6")
    with open(log_path, "w", encoding="utf-8"):
        pass
    apply_text(json.dumps(v6_snat_plan()), False)
    v6_log = _load_log(log_path)
    snat = [argv for argv in v6_log if "SNAT" in argv and "fd00:66::9" in argv]
    _assert(snat, "private ipv6 external snat")
    _assert(all(_tool(argv, "ip6tables") for argv in snat), "ipv6 snat uses ip6tables")
    _assert(any("fd00:90::/64" in argv for argv in snat), "ipv6 snat matches the tenant prefix")
    _assert(all("ns0123456789ac" in argv for argv in snat), "ipv6 snat stays in the router netns")
    _assert(
        any("net.ipv6.conf.all.forwarding=1" in argv for argv in v6_log),
        "ipv6 forwarding in the router netns",
    )

    gone = os.path.join(td, "bin-gone")
    os.makedirs(gone, mode=0o755)
    for name in ("ip", "bridge", "iptables", "sysctl", "dnsmasq", "ip6tables"):
        path = os.path.join(gone, name)
        lines = [
            "#!/usr/bin/env python3",
            "import json, sys",
            f"with open({log_path!r}, 'a', encoding='utf-8') as fh:",
            '    fh.write(json.dumps(sys.argv) + "\\n")',
            "argv = sys.argv[1:]",
            'if "link" in argv and "show" in argv:',
            '    sys.stderr.write("Device does not exist\\n")',
            "    sys.exit(1)",
            'if ("link" in argv and "del" in argv) or ("netns" in argv and "del" in argv):',
            '    sys.stderr.write("Cannot find device\\n")',
            "    sys.exit(1)",
            'if "fdb" in argv and "del" in argv:',
            '    sys.stderr.write("No such file or directory\\n")',
            "    sys.exit(1)",
            'if "-D" in argv:',
            '    sys.stderr.write("Bad rule (does a matching rule exist in that chain?)\\n")',
            "    sys.exit(1)",
            "sys.exit(0)",
            "",
        ]
        with open(path, "w", encoding="utf-8") as fh:
            fh.write("\n".join(lines))
        os.chmod(path, 0o755)
    os.environ["PISTACK_OVERLAY_IP"] = os.path.join(gone, "ip")
    os.environ["PISTACK_OVERLAY_BRIDGE"] = os.path.join(gone, "bridge")
    os.environ["PISTACK_OVERLAY_IPTABLES"] = os.path.join(gone, "iptables")
    os.environ["PISTACK_OVERLAY_IP6TABLES"] = os.path.join(gone, "ip6tables")
    os.environ["PISTACK_OVERLAY_SYSCTL"] = os.path.join(gone, "sysctl")
    state = os.path.join(td, "state-gone")
    os.environ["PISTACK_OVERLAY_STATE"] = state
    os.makedirs(state, mode=0o755)
    with open(os.path.join(state, "state.json"), "w", encoding="utf-8") as fh:
        json.dump(
            {
                "version": 1,
                "created": [
                    "po01a10318a507",
                    "vx01a10318a507",
                    "ns0123456789ab",
                    "nf0123456789ab",
                    "a01a103012345",
                    "b01a103012345",
                    "e0123456789ab",
                    "f0123456789ab",
                ],
                "fdb": [
                    {"dev": "vx01a10318a507", "mac": "00:00:00:00:00:00", "dst": "10.0.0.2"}
                ],
                "routes": [],
                "veths": [
                    {
                        "root": "a01a103012345",
                        "peer": "b01a103012345",
                        "netns": "ns0123456789ab",
                    },
                    {
                        "root": "e0123456789ab",
                        "peer": "f0123456789ab",
                        "netns": "nf0123456789ab",
                    },
                ],
                "rules": [
                    {
                        "netns": "ns0123456789ab",
                        "table": "nat",
                        "chain": "POSTROUTING",
                        "spec": [
                            "-s",
                            "10.90.0.0/24",
                            "-o",
                            "d0123456789ab",
                            "-j",
                            "SNAT",
                            "--to-source",
                            "10.66.66.84",
                        ],
                    }
                ],
                "addrs": [],
            },
            fh,
        )
    apply_text(json.dumps(empty), False)
    apply_text(json.dumps(empty), False)
    with open(os.path.join(state, "state.json"), encoding="utf-8") as fh:
        left = json.load(fh)
    _assert(left["created"] == [], "gone devices are dropped from state")
    _assert(left["veths"] == [], "gone veths are dropped from state")
    _assert(left["fdb"] == [], "gone fdb is dropped from state")
    _assert(left["rules"] == [], "gone rules are dropped from state")
    bindir = os.path.join(td, "bin")
    os.environ["PISTACK_OVERLAY_IP"] = os.path.join(bindir, "ip")
    os.environ["PISTACK_OVERLAY_BRIDGE"] = os.path.join(bindir, "bridge")
    os.environ["PISTACK_OVERLAY_IPTABLES"] = os.path.join(bindir, "iptables")
    os.environ["PISTACK_OVERLAY_IP6TABLES"] = os.path.join(bindir, "ip6tables")
    os.environ["PISTACK_OVERLAY_SYSCTL"] = os.path.join(bindir, "sysctl")
    os.environ["PISTACK_OVERLAY_DNSMASQ"] = os.path.join(bindir, "dnsmasq")


def self_test() -> None:
    """Apply a two-network plan through fake ip/bridge/iptables/sysctl."""
    keys = (
        "PISTACK_OVERLAY_STATE",
        "PISTACK_OVERLAY_IP",
        "PISTACK_OVERLAY_BRIDGE",
        "PISTACK_OVERLAY_IPTABLES",
        "PISTACK_OVERLAY_IP6TABLES",
        "PISTACK_OVERLAY_SYSCTL",
        "PISTACK_OVERLAY_DNSMASQ",
    )
    saved = {key: os.environ.get(key) for key in keys}
    td = tempfile.mkdtemp(prefix="pistack-overlay-selftest-")
    try:
        log_path = os.path.join(td, "log")
        bindir = os.path.join(td, "bin")
        _install_fakes(bindir, log_path)
        os.environ["PISTACK_OVERLAY_STATE"] = os.path.join(td, "state")
        os.environ["PISTACK_OVERLAY_IP"] = os.path.join(bindir, "ip")
        os.environ["PISTACK_OVERLAY_BRIDGE"] = os.path.join(bindir, "bridge")
        os.environ["PISTACK_OVERLAY_IPTABLES"] = os.path.join(bindir, "iptables")
        os.environ["PISTACK_OVERLAY_IP6TABLES"] = os.path.join(bindir, "ip6tables")
        os.environ["PISTACK_OVERLAY_SYSCTL"] = os.path.join(bindir, "sysctl")
        os.environ["PISTACK_OVERLAY_DNSMASQ"] = os.path.join(bindir, "dnsmasq")
        _check_refusals()
        with open(log_path, "w", encoding="utf-8"):
            pass
        try:
            apply_text(json.dumps(sample_plan(True)), False)
            mark = _load_log(log_path)
            apply_text(json.dumps(sample_plan(False)), False)
            full = _load_log(log_path)
            os.environ["PISTACK_OVERLAY_STATE"] = os.path.join(td, "state-float")
            before = len(full)
            apply_text(json.dumps(floating_plan(True)), False)
            mid = _load_log(log_path)
            apply_text(json.dumps(floating_plan(False)), False)
            end = _load_log(log_path)
        except SystemExit as exc:
            if exc.code == 1:
                raise
            fail(f"self-test: apply exited {exc.code}")
        _check_log(mark, full[len(mark) :])
        _check_floating(mid[before:], end[len(mid) :])
        _check_delete_and_v6(td, log_path)
        confs = [
            name
            for name in os.listdir(os.path.join(td, "state"))
            if name.startswith("dhcp-") and name.endswith(".conf")
        ]
        _assert(confs, "dhcp config rendered")
        conf_text = open(os.path.join(td, "state", confs[0]), encoding="utf-8").read()
        _assert("dhcp-host=" in conf_text and "static" in conf_text, "dhcp config body")
        os.environ["PISTACK_OVERLAY_STATE"] = os.path.join(td, "state-missing")
        os.environ["PISTACK_OVERLAY_DNSMASQ"] = os.path.join(td, "missing-dnsmasq")
        held = sys.stderr
        sys.stderr = io.StringIO()
        try:
            apply_text(json.dumps(sample_plan(False)), False)
        except SystemExit as exc:
            sys.stderr = held
            _assert(exc.code == 1, "missing dnsmasq exits 1")
        else:
            sys.stderr = held
            fail("self-test: missing dnsmasq was accepted")
        finally:
            sys.stderr = held
    finally:
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        shutil.rmtree(td, ignore_errors=True)
    print("pistack-overlay: self-test ok")


def main(argv: Optional[list[str]] = None) -> None:
    parser = argparse.ArgumentParser(prog="pistack-overlay")
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="converge a sample plan through fake ip, bridge, iptables, and sysctl",
    )
    sub = parser.add_subparsers(dest="cmd")
    apply_p = sub.add_parser("apply", help="converge the host to a plan file")
    apply_p.add_argument("--plan", required=True, help="plan JSON path, or - for stdin")
    apply_p.add_argument("--dry-run", action="store_true", help="print commands and change nothing")
    args = parser.parse_args(argv)
    if args.self_test:
        self_test()
        return
    if args.cmd == "apply":
        cmd_apply(args.plan, args.dry_run)
        return
    die("a command is required")


if __name__ == "__main__":
    main()
