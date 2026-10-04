#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 P. Industries LLC and PiStack contributors
"""Install or remove one floating-IP NAT on the fleet WireGuard interface.

The address must come from 10.66.66.80/28. It is added on wg-fleet only, and
DNAT accepts traffic sourced from 10.66.66.0/24. The public NIC is never
given the address.

    pistack-fip ensure --id fip-... --floating 10.66.66.82 --fixed 10.88.0.187
    pistack-fip delete --id fip-...
    pistack-fip ensure ... --dry-run
    pistack-fip sg --fixed 10.88.0.187 --policy - <<'JSON'
    {"ingress":[{"proto":"tcp","port_min":22,"port_max":22,"cidr":"10.66.66.0/24"}],"egress":[]}
    JSON
    pistack-fip sg --fixed 10.88.0.187 --clear
    pistack-fip guest-cidr add --cidr 10.90.0.0/24
    pistack-fip metadata-on --bridge pistack0
    pistack-fip restore
    pistack-fip --self-test

Security-group rules live in the filter table, chain PISTACK-SG, and use
comment prefix pistack-sg-<fixed>. ensure still installs the blanket
FORWARD accept. sg removes that accept only after a non-empty ingress
policy is in place. --dry-run prints commands and changes nothing.

Fixed addresses must sit in 10.88.0.0/24 or a CIDR added to
$PISTACK_FIP_STATE/guest-cidrs. metadata-on puts 169.254.169.254/32 on
that bridge, sets route_localnet only there, and DNATs tcp/80 to
127.0.0.1:18880. filter INPUT accepts tcp/18880 only from that bridge.
It does not DNAT on wg-fleet and it does not open the public NIC.
metadata-on records that bridge, once, in $PISTACK_FIP_STATE/bridges.
restore replays stored floating IPs, security groups, and those bridges.
guest-cidrs is left as stored. A missing or empty state directory prints
a line and adds no rules.
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
import subprocess
import sys
import tempfile
from typing import NoReturn, Optional

POOL = ipaddress.ip_network("10.66.66.80/28")
GUESTS = ipaddress.ip_network("10.88.0.0/24")
SOURCE = ipaddress.ip_network("10.66.66.0/24")
IFACE = "wg-fleet"
GATEWAY = ipaddress.ip_address("10.88.0.1")
HOST = ipaddress.ip_address("10.66.66.1")
# Shorter than this is a fleet-sized (or larger) network, not a guest subnet.
MIN_GUEST_PREFIX = 20
METADATA_ADDR = "169.254.169.254"
METADATA_COMMENT = "pistack-metadata"
DEFAULT_STATE_DIR = "/var/lib/pistack/fip"
ID_RE = re.compile(
    r"^fip-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
)
BRIDGE_RE = re.compile(r"^(?:pistack0|pi[0-9a-f]{8,12})$")

PROTOCOLS = ("tcp", "udp", "icmp")
SG_CHAIN = "PISTACK-SG"
SG_PROTOS = ("tcp", "udp", "icmp", "any")


def die(msg: str) -> NoReturn:
    print(f"pistack-fip: {msg}", file=sys.stderr)
    raise SystemExit(2)


def parse_ip(text: str, what: str) -> ipaddress.IPv4Address:
    try:
        ip = ipaddress.ip_address(text)
    except ValueError:
        die(f"{what} {text} is not an address")
    if not isinstance(ip, ipaddress.IPv4Address):
        die(f"{what} must be ipv4")
    return ip


def check_id(value: str) -> str:
    if not ID_RE.match(value):
        die("id must be a fip- uuid")
    return value


def check_floating(ip: ipaddress.IPv4Address) -> None:
    if ip not in POOL or ip in (POOL.network_address, POOL.broadcast_address):
        die(f"{ip} is outside the floating pool {POOL}")
    if ip == HOST or ip in peer_addrs():
        die(f"{ip} is already a fleet peer address")


def state_dir() -> str:
    """Directory for fip, sg, guest-cidr, and bridge state. Honors PISTACK_FIP_STATE."""
    return os.environ.get("PISTACK_FIP_STATE", DEFAULT_STATE_DIR)


def check_fixed(ip: ipaddress.IPv4Address) -> None:
    allowed = allowed_guest_cidrs()
    # 10.88.0.1 stays refused even if a later CIDR would otherwise contain it.
    if ip != GATEWAY and any(fixed_usable(ip, net) for net in allowed):
        return
    shown = ", ".join(str(net) for net in allowed)
    die(f"{ip} is not a guest address on {shown}")


def fixed_usable(ip: ipaddress.IPv4Address, net: ipaddress.IPv4Network) -> bool:
    """True when `ip` is a host in `net`, not the network, broadcast, or .1."""
    if ip not in net:
        return False
    if ip == net.network_address or ip == net.broadcast_address:
        return False
    gw = conventional_gateway(net)
    return gw is None or ip != gw


def conventional_gateway(
    net: ipaddress.IPv4Network,
) -> Optional[ipaddress.IPv4Address]:
    """network+1 when that address is still inside the CIDR."""
    try:
        gw = net.network_address + 1
    except ipaddress.AddressValueError:
        return None
    if gw in net:
        return gw
    return None


def guest_cidr_path() -> str:
    return os.path.join(state_dir(), "guest-cidrs")


def parse_guest_cidr(text: str) -> ipaddress.IPv4Network:
    try:
        net = ipaddress.ip_network(text, strict=True)
    except ValueError:
        die(f"{text} is not an ipv4 network")
    if not isinstance(net, ipaddress.IPv4Network):
        die(f"{text} must be ipv4")
    return net


def refuse_guest_cidr(net: ipaddress.IPv4Network) -> None:
    if net.prefixlen < MIN_GUEST_PREFIX:
        die(f"{net} prefix length is under {MIN_GUEST_PREFIX}")
    if HOST in net:
        die(f"{net} contains {HOST}")
    if net.overlaps(POOL):
        die(f"{net} overlaps {POOL}")
    if net.overlaps(SOURCE):
        die(f"{net} overlaps {SOURCE}")


def stored_guest_cidrs() -> list[ipaddress.IPv4Network]:
    path = guest_cidr_path()
    if not os.path.exists(path):
        return []
    try:
        with open(path, encoding="utf-8") as fh:
            lines = fh.readlines()
    except OSError as exc:
        die(f"cannot read {path}: {exc}")
    found: list[ipaddress.IPv4Network] = []
    for lineno, line in enumerate(lines, 1):
        text = line.strip()
        if not text:
            continue
        try:
            net = ipaddress.ip_network(text, strict=True)
        except ValueError:
            die(f"{path}:{lineno}: {text} is not an ipv4 network")
        if not isinstance(net, ipaddress.IPv4Network):
            die(f"{path}:{lineno}: {text} must be ipv4")
        if net not in found:
            found.append(net)
    return found


def allowed_guest_cidrs() -> list[ipaddress.IPv4Network]:
    """10.88.0.0/24 is always allowed, even when guest-cidrs is missing."""
    found = [GUESTS]
    for net in stored_guest_cidrs():
        if net not in found:
            found.append(net)
    return found


def write_guest_cidrs(nets: list[ipaddress.IPv4Network]) -> None:
    os.makedirs(state_dir(), mode=0o755, exist_ok=True)
    path = guest_cidr_path()
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        for net in nets:
            fh.write(f"{net}\n")
    os.chmod(tmp, 0o644)
    os.replace(tmp, path)


def add_guest_cidr(text: str) -> None:
    net = parse_guest_cidr(text)
    refuse_guest_cidr(net)
    current = stored_guest_cidrs()
    if net in current:
        return
    # The builtin is already allowed. Recording it is fine; never drop it.
    current.append(net)
    write_guest_cidrs(current)


def check_bridge(name: str) -> str:
    if BRIDGE_RE.fullmatch(name):
        return name
    die(f"bridge {name} is not allowed")


def dev_has_metadata(dev: str) -> bool:
    try:
        out = subprocess.run(
            ["ip", "-4", "-o", "addr", "show", "dev", dev],
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError:
        return False
    if out.returncode != 0:
        return False
    want = ipaddress.ip_address(METADATA_ADDR)
    for tok in out.stdout.replace("/", " ").split():
        try:
            found = ipaddress.ip_address(tok)
        except ValueError:
            continue
        if isinstance(found, ipaddress.IPv4Address) and found == want:
            return True
    return False


def metadata_dnat_spec(bridge: str) -> list[str]:
    """PREROUTING match that sends metadata to the loopback listener."""
    return [
        "-i",
        bridge,
        "-d",
        f"{METADATA_ADDR}/32",
        "-p",
        "tcp",
        "--dport",
        "80",
        "-m",
        "comment",
        "--comment",
        METADATA_COMMENT,
        "-j",
        "DNAT",
        "--to-destination",
        "127.0.0.1:18880",
    ]


def metadata_redirect_specs(bridge: str) -> list[list[str]]:
    """Old REDIRECT forms. metadata-on deletes these; it does not install them.

    REDIRECT rewrites the destination to the bridge address, where nothing
    listens. Both the bare address and the /32 form are removed.
    """
    common = [
        "-i",
        bridge,
        "-p",
        "tcp",
        "--dport",
        "80",
        "-m",
        "comment",
        "--comment",
        METADATA_COMMENT,
        "-j",
        "REDIRECT",
        "--to-ports",
        "18880",
    ]
    specs = []
    for dest in (METADATA_ADDR, f"{METADATA_ADDR}/32"):
        spec = common.copy()
        spec[2:2] = ["-d", dest]
        specs.append(spec)
    return specs


def metadata_input_spec(bridge: str) -> list[str]:
    """filter INPUT accept for the DNATed metadata port, one bridge only."""
    return [
        "-i",
        bridge,
        "-p",
        "tcp",
        "--dport",
        "18880",
        "-m",
        "comment",
        "--comment",
        METADATA_COMMENT,
        "-j",
        "ACCEPT",
    ]


def metadata_sysctl(bridge: str) -> list[str]:
    # Per-bridge only. all.route_localnet stays unset.
    return ["sysctl", "-w", f"net.ipv4.conf.{bridge}.route_localnet=1"]


def iptables_has(table: str, chain: str, spec: list[str]) -> bool:
    probe = subprocess.run(
        ["iptables", "-t", table, "-C", chain, *spec],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return probe.returncode == 0


def bridges_path() -> str:
    return os.path.join(state_dir(), "bridges")


def read_bridges() -> list[str]:
    """Bridge names recorded for restore, in file order. Missing file is empty."""
    path = bridges_path()
    if not os.path.exists(path):
        return []
    try:
        with open(path, encoding="utf-8") as fh:
            lines = fh.readlines()
    except OSError as exc:
        die(f"cannot read {path}: {exc}")
    found: list[str] = []
    for lineno, line in enumerate(lines, 1):
        text = line.strip()
        if not text or text in found:
            continue
        if not BRIDGE_RE.fullmatch(text):
            die(f"{path}:{lineno}: bridge {text} is not allowed")
        found.append(text)
    return found


def write_bridges(names: list[str]) -> None:
    os.makedirs(state_dir(), mode=0o755, exist_ok=True)
    path = bridges_path()
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        for name in names:
            fh.write(f"{name}\n")
    os.chmod(tmp, 0o644)
    os.replace(tmp, path)


def record_metadata_bridge(bridge: str) -> str:
    """Append one allowed bridge. The same name is not written twice."""
    name = check_bridge(bridge)
    found = read_bridges()
    if name in found:
        return name
    found.append(name)
    write_bridges(found)
    return name


def metadata_on(bridge: str) -> None:
    """Send one pistack bridge's metadata address to 127.0.0.1:18880.

    The guest routes 169.254.169.254 via its gateway, so the packet arrives
    on the bridge already. DNAT in PREROUTING aims it at the loopback
    listener. route_localnet is enabled on that bridge only, and filter
    INPUT accepts tcp/18880 only from that bridge. The previous REDIRECT
    rule is removed. Nothing is opened on wg-fleet or the public NIC.
    The name is recorded for restore only after check_bridge accepts it.
    """
    _apply_metadata(record_metadata_bridge(bridge))


def _apply_metadata(name: str) -> None:
    if not dev_has_metadata(name):
        run(["ip", "addr", "add", f"{METADATA_ADDR}/32", "dev", name], False)
    run(metadata_sysctl(name), False)
    dnat = metadata_dnat_spec(name)
    if not iptables_has("nat", "PREROUTING", dnat):
        run(["iptables", "-t", "nat", "-I", "PREROUTING", "1", *dnat], False)
    for spec in metadata_redirect_specs(name):
        drop_rule("nat", "PREROUTING", spec, False)
    inbound = metadata_input_spec(name)
    if not iptables_has("filter", "INPUT", inbound):
        run(["iptables", "-I", "INPUT", "1", *inbound], False)


def peer_addrs() -> set[ipaddress.IPv4Address]:
    try:
        out = subprocess.run(
            ["wg", "show", IFACE, "allowed-ips"],
            check=False,
            capture_output=True,
            text=True,
        ).stdout
    except OSError:
        return set()
    found: set[ipaddress.IPv4Address] = set()
    for line in out.splitlines():
        for tok in line.replace("=", " ").replace(",", " ").split():
            tok = tok.split("/")[0]
            try:
                ip = ipaddress.ip_address(tok)
            except ValueError:
                continue
            if isinstance(ip, ipaddress.IPv4Address):
                found.add(ip)
    return found


def state_path(fip_id: str) -> str:
    return os.path.join(state_dir(), fip_id)


def read_state(fip_id: str) -> Optional[dict]:
    path = state_path(fip_id)
    if not os.path.exists(path):
        return None
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


def write_state(fip_id: str, floating: str, fixed: str) -> None:
    os.makedirs(state_dir(), mode=0o755, exist_ok=True)
    path = state_path(fip_id)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump({"floating": floating, "fixed": fixed}, fh)
    os.chmod(tmp, 0o644)
    os.replace(tmp, path)


def comment_for(fip_id: str) -> str:
    return f"pistack-{fip_id}"


def nat_specs(fip_id: str, floating: str, fixed: str) -> list[tuple[str, str, list[str]]]:
    """(table, chain, args) for every rule this floating IP owns."""
    mark = comment_for(fip_id)
    rules = []
    for proto in PROTOCOLS:
        rules.append(
            (
                "nat",
                "PREROUTING",
                [
                    "-s",
                    str(SOURCE),
                    "-d",
                    f"{floating}/32",
                    "-p",
                    proto,
                    "-m",
                    "comment",
                    "--comment",
                    mark,
                    "-j",
                    "DNAT",
                    "--to-destination",
                    fixed,
                ],
            )
        )
        rules.append(
            (
                "nat",
                "POSTROUTING",
                [
                    "-s",
                    f"{fixed}/32",
                    "-d",
                    str(SOURCE),
                    "-p",
                    proto,
                    "-m",
                    "comment",
                    "--comment",
                    mark,
                    "-j",
                    "SNAT",
                    "--to-source",
                    floating,
                ],
            )
        )
    rules.append(
        (
            "filter",
            "FORWARD",
            [
                "-s",
                str(SOURCE),
                "-d",
                f"{fixed}/32",
                "-m",
                "comment",
                "--comment",
                mark,
                "-j",
                "ACCEPT",
            ],
        )
    )
    return rules


def run(argv: list[str], dry: bool) -> None:
    if dry:
        print(" ".join(argv))
        return
    subprocess.run(argv, check=True)


def drop_rule(table: str, chain: str, spec: list[str], dry: bool) -> None:
    cmd = ["iptables", "-t", table, "-D", chain, *spec]
    if dry:
        print(" ".join(cmd))
        return
    while subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
        pass


def ensure(fip_id: str, floating: str, fixed: str, dry: bool) -> None:
    check_floating(parse_ip(floating, "floating"))
    check_fixed(parse_ip(fixed, "fixed"))
    previous = read_state(fip_id)
    if previous and (
        previous.get("floating") != floating or previous.get("fixed") != fixed
    ):
        remove(fip_id, previous["floating"], previous["fixed"], dry)
    # A non-empty group policy removes this accept. Do not put it back
    # on a later ensure, or it sits behind the group chain and looks open.
    group_installed = os.path.exists(sg_state_path(fixed))
    for table, chain, spec in nat_specs(fip_id, floating, fixed):
        drop_rule(table, chain, spec, dry)
        if group_installed and table == "filter":
            continue
        run(["iptables", "-t", table, "-I", chain, "1", *spec], dry)
    run(["ip", "addr", "replace", f"{floating}/32", "dev", IFACE], dry)
    if not dry:
        write_state(fip_id, floating, fixed)
        # Blanket FORWARD accepts are inserted at position 1. If a group
        # chain is already installed, put its jump back in front of them.
        pin_sg_jump()


def del_addr(floating: str, dry: bool) -> None:
    cmd = ["ip", "addr", "del", f"{floating}/32", "dev", IFACE]
    if dry:
        print(" ".join(cmd))
        return
    # Already gone is success. ensure() hits this when the pair changes.
    subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def remove(fip_id: str, floating: str, fixed: str, dry: bool) -> None:
    for table, chain, spec in nat_specs(fip_id, floating, fixed):
        drop_rule(table, chain, spec, dry)
    del_addr(floating, dry)
    if not dry:
        try:
            os.remove(state_path(fip_id))
        except FileNotFoundError:
            pass


def delete(fip_id: str, dry: bool) -> None:
    state = read_state(fip_id)
    if not state:
        return
    # addr del returns non-zero when the address is already gone. Ignore that.
    floating = state["floating"]
    fixed = state["fixed"]
    for table, chain, spec in nat_specs(fip_id, floating, fixed):
        drop_rule(table, chain, spec, dry)
    del_addr(floating, dry)
    # Group rules are keyed by the fixed IP. No saved policy is a no-op,
    # so delete still succeeds when this address was never given one.
    clear_sg(fixed, dry)
    if not dry:
        try:
            os.remove(state_path(fip_id))
        except FileNotFoundError:
            pass


def comment_prefix(fixed: str) -> str:
    return f"pistack-sg-{fixed}"


def sg_state_path(fixed: str) -> str:
    return os.path.join(state_dir(), f"sg-{fixed}")


def comment_of(spec: list[str]) -> str:
    for i, tok in enumerate(spec):
        if tok == "--comment" and i + 1 < len(spec):
            return spec[i + 1]
    return ""


def fixed_of_sg_comment(comment: str) -> Optional[str]:
    """IP embedded in `pistack-sg-<ip>-<suffix>`. IPv4 has no dash, so the
    address is everything up to the next dash. A startswith match would
    treat 10.88.0.18 as owning 10.88.0.187.
    """
    prefix = "pistack-sg-"
    if not comment.startswith(prefix):
        return None
    ip, sep, _suffix = comment[len(prefix) :].partition("-")
    if not ip or not sep:
        return None
    return ip


def jump_target(spec: list[str]) -> Optional[str]:
    for i, tok in enumerate(spec):
        if tok in ("-j", "--jump") and i + 1 < len(spec):
            return spec[i + 1]
    return None


def has_match(spec: list[str], flag: str, value: str) -> bool:
    aliases = {flag}
    if flag == "-d":
        aliases.add("--destination")
    if flag == "-s":
        aliases.add("--source")
    for i, tok in enumerate(spec):
        if tok in aliases and i + 1 < len(spec) and spec[i + 1] == value:
            return True
    return False


def parse_rule_line(line: str, chain: str) -> Optional[list[str]]:
    if not line.startswith(f"-A {chain} "):
        return None
    try:
        parts = shlex.split(line)
    except ValueError:
        return None
    if len(parts) < 2 or parts[1] != chain:
        return None
    return parts[2:]


def iptables_save(chain: str) -> list[str]:
    try:
        out = subprocess.run(
            ["iptables", "-t", "filter", "-S", chain],
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError:
        return []
    if out.returncode != 0:
        return []
    return out.stdout.splitlines()


def sg_jump_is_first() -> bool:
    for line in iptables_save("FORWARD"):
        if line.startswith("-A FORWARD "):
            return line.strip() == f"-A FORWARD -j {SG_CHAIN}"
    return False


def pin_sg_jump() -> None:
    """Move the PISTACK-SG jump to the front of FORWARD if the chain exists."""
    probe = subprocess.run(
        ["iptables", "-t", "filter", "-S", SG_CHAIN],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if probe.returncode != 0 or sg_jump_is_first():
        return
    delete_jump = ["iptables", "-t", "filter", "-D", "FORWARD", "-j", SG_CHAIN]
    while (
        subprocess.run(delete_jump, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode
        == 0
    ):
        pass
    subprocess.run(
        ["iptables", "-t", "filter", "-I", "FORWARD", "1", "-j", SG_CHAIN],
        check=True,
    )


def install_sg_chain(dry: bool) -> None:
    create = ["iptables", "-t", "filter", "-N", SG_CHAIN]
    delete_jump = ["iptables", "-t", "filter", "-D", "FORWARD", "-j", SG_CHAIN]
    insert_jump = ["iptables", "-t", "filter", "-I", "FORWARD", "1", "-j", SG_CHAIN]
    if dry:
        print(" ".join(create))
        print(" ".join(delete_jump))
        print(" ".join(insert_jump))
        return
    created = subprocess.run(create, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    if created.returncode != 0:
        probe = subprocess.run(
            ["iptables", "-t", "filter", "-S", SG_CHAIN],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        if probe.returncode != 0:
            die((created.stderr or "cannot create PISTACK-SG").strip())
    if not sg_jump_is_first():
        while (
            subprocess.run(
                delete_jump, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
            ).returncode
            == 0
        ):
            pass
        subprocess.run(insert_jump, check=True)


def load_saved_specs(fixed: str) -> list[list[str]]:
    path = sg_state_path(fixed)
    if not os.path.exists(path):
        return []
    try:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, json.JSONDecodeError):
        return []
    rules = data.get("rules", [])
    if not isinstance(rules, list):
        return []
    specs: list[list[str]] = []
    for spec in rules:
        if isinstance(spec, list) and all(isinstance(p, str) for p in spec):
            specs.append(spec)
    return specs


def live_sg_specs(fixed: str) -> list[list[str]]:
    found: list[list[str]] = []
    for line in iptables_save(SG_CHAIN):
        spec = parse_rule_line(line, SG_CHAIN)
        if spec is None:
            continue
        if fixed_of_sg_comment(comment_of(spec)) == fixed:
            found.append(spec)
    return found


def remove_sg_rules(fixed: str, dry: bool, unlink: bool = True) -> None:
    specs = load_saved_specs(fixed)
    if not dry:
        seen = {tuple(s) for s in specs}
        for spec in live_sg_specs(fixed):
            if tuple(spec) not in seen:
                specs.append(spec)
    for spec in specs:
        drop_rule("filter", SG_CHAIN, spec, dry)
    # clear drops the saved policy. restore keeps the file and reinserts it.
    if not dry and unlink:
        try:
            os.remove(sg_state_path(fixed))
        except FileNotFoundError:
            pass


def clear_sg(fixed: str, dry: bool) -> None:
    """Drop this address's group rules. Does not re-add the blanket accept."""
    remove_sg_rules(fixed, dry)


def write_sg_state(fixed: str, specs: list[list[str]]) -> None:
    os.makedirs(state_dir(), mode=0o755, exist_ok=True)
    path = sg_state_path(fixed)
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as fh:
        json.dump({"fixed": fixed, "rules": specs}, fh)
    os.chmod(tmp, 0o644)
    os.replace(tmp, path)


def fip_ids_for_fixed(fixed: str) -> list[tuple[str, str]]:
    """(fip id, floating) recorded for this guest address."""
    root = state_dir()
    if not os.path.isdir(root):
        return []
    found: list[tuple[str, str]] = []
    for name in sorted(os.listdir(root)):
        if not name.startswith("fip-"):
            continue
        try:
            state = read_state(name)
        except (OSError, json.JSONDecodeError):
            continue
        if not state or state.get("fixed") != fixed:
            continue
        floating = state.get("floating")
        if isinstance(floating, str):
            found.append((name, floating))
    return found


def blanket_comment(comment: str) -> bool:
    """True for this helper's own filter accept (`pistack-fip-<id>`).

    `pistack-demo-ssh` is the separate port-22187 exception and must stay.
    Group rules use `pistack-sg-` and are removed by `remove_sg_rules`.
    """
    return comment.startswith("pistack-fip-")


def live_blanket_accepts(fixed: str) -> list[list[str]]:
    dest = f"{fixed}/32"
    found: list[list[str]] = []
    for line in iptables_save("FORWARD"):
        spec = parse_rule_line(line, "FORWARD")
        if spec is None:
            continue
        if jump_target(spec) != "ACCEPT":
            continue
        if not has_match(spec, "-d", dest):
            continue
        if blanket_comment(comment_of(spec)):
            found.append(spec)
    return found


def remove_blanket_accepts(fixed: str, dry: bool) -> None:
    """Drop filter FORWARD accepts for this fixed /32. NAT rules stay."""
    for fip_id, floating in fip_ids_for_fixed(fixed):
        for table, chain, spec in nat_specs(fip_id, floating, fixed):
            if table == "filter" and chain == "FORWARD":
                drop_rule(table, chain, spec, dry)
    if dry:
        return
    for spec in live_blanket_accepts(fixed):
        drop_rule("filter", "FORWARD", spec, False)


class SgRule:
    def __init__(
        self,
        proto: str,
        port_min: Optional[int],
        port_max: Optional[int],
        cidr: Optional[str],
    ) -> None:
        self.proto = proto
        self.port_min = port_min
        self.port_max = port_max
        self.cidr = cidr


def parse_port(value: object) -> Optional[int]:
    if isinstance(value, bool) or not isinstance(value, int):
        return None
    if not 1 <= value <= 65535:
        return None
    return value


def parse_cidr(value: object) -> Optional[str]:
    if not isinstance(value, str) or not value or "/" not in value:
        return None
    try:
        net = ipaddress.ip_network(value, strict=False)
    except ValueError:
        return None
    if not isinstance(net, ipaddress.IPv4Network):
        return None
    return str(net)


def parse_rule(item: object) -> Optional[SgRule]:
    if not isinstance(item, dict):
        return None
    proto = item.get("proto", "any")
    if not isinstance(proto, str):
        return None
    proto = proto.lower()
    if proto not in SG_PROTOS:
        return None
    cidr: Optional[str] = None
    if "cidr" in item and item.get("cidr") not in (None, ""):
        cidr = parse_cidr(item.get("cidr"))
        if cidr is None:
            return None
    port_min: Optional[int] = None
    port_max: Optional[int] = None
    if proto in ("tcp", "udp"):
        raw_min = item.get("port_min") if "port_min" in item else None
        raw_max = item.get("port_max") if "port_max" in item else None
        if raw_min is not None:
            port_min = parse_port(raw_min)
            if port_min is None:
                return None
        if raw_max is not None:
            port_max = parse_port(raw_max)
            if port_max is None:
                return None
        if port_min is None:
            port_min = port_max
        if port_max is None:
            port_max = port_min
        if port_min is not None and port_max is not None and port_min > port_max:
            return None
    return SgRule(proto, port_min, port_max, cidr)


def parse_rule_list(items: object, what: str) -> list[SgRule]:
    if items is None:
        return []
    if not isinstance(items, list):
        die(f"{what} must be a list")
    rules: list[SgRule] = []
    for item in items:
        rule = parse_rule(item)
        if rule is not None:
            rules.append(rule)
    return rules


def parse_policy(raw: str) -> tuple[list[SgRule], list[SgRule]]:
    if not raw.strip():
        die("policy is empty")
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        die(f"policy is not json: {exc}")
    if not isinstance(data, dict):
        die("policy must be a json object")
    return parse_rule_list(data.get("ingress", []), "ingress"), parse_rule_list(
        data.get("egress", []), "egress"
    )


def ct_spec(fixed: str, direction: str, comment: str) -> list[str]:
    spec = ["-d", f"{fixed}/32"] if direction == "in" else ["-s", f"{fixed}/32"]
    spec += [
        "-m",
        "conntrack",
        "--ctstate",
        "ESTABLISHED,RELATED",
        "-m",
        "comment",
        "--comment",
        comment,
        "-j",
        "ACCEPT",
    ]
    return spec


def drop_spec(fixed: str, direction: str, comment: str) -> list[str]:
    if direction == "in":
        spec = ["-d", f"{fixed}/32"]
    else:
        spec = ["-s", f"{fixed}/32", "-m", "conntrack", "--ctstate", "NEW"]
    spec += ["-m", "comment", "--comment", comment, "-j", "DROP"]
    return spec


def accept_spec(fixed: str, direction: str, rule: SgRule, comment: str) -> list[str]:
    spec = ["-d", f"{fixed}/32"] if direction == "in" else ["-s", f"{fixed}/32"]
    if rule.proto in ("tcp", "udp"):
        spec += ["-p", rule.proto, "-m", rule.proto]
        if rule.port_min is not None and rule.port_max is not None:
            port = (
                str(rule.port_min)
                if rule.port_min == rule.port_max
                else f"{rule.port_min}:{rule.port_max}"
            )
            spec += ["--dport", port]
    elif rule.proto == "icmp":
        spec += ["-p", "icmp"]
    if rule.cidr:
        spec += ["-s", rule.cidr] if direction == "in" else ["-d", rule.cidr]
    spec += ["-m", "comment", "--comment", comment, "-j", "ACCEPT"]
    return spec


def policy_specs(fixed: str, ingress: list[SgRule], egress: list[SgRule]) -> list[list[str]]:
    prefix = comment_prefix(fixed)
    specs = [
        ct_spec(fixed, "in", f"{prefix}-ct-in"),
        ct_spec(fixed, "out", f"{prefix}-ct-out"),
    ]
    for i, rule in enumerate(ingress):
        specs.append(accept_spec(fixed, "in", rule, f"{prefix}-in-{i}"))
    if ingress:
        specs.append(drop_spec(fixed, "in", f"{prefix}-in-drop"))
    for i, rule in enumerate(egress):
        specs.append(accept_spec(fixed, "out", rule, f"{prefix}-out-{i}"))
    if egress:
        specs.append(drop_spec(fixed, "out", f"{prefix}-out-drop"))
    return specs


def install_sg_specs(
    fixed: str,
    specs: list[list[str]],
    dry: bool,
    drop_blanket: bool,
    unlink: bool = True,
) -> None:
    """Put already-built group rules in front of PISTACK-SG."""
    install_sg_chain(dry)
    remove_sg_rules(fixed, dry, unlink=unlink)
    # Insert at position 1 in reverse so `specs` ends up in order, in front
    # of every other address. Other addresses' rules are not deleted.
    for spec in reversed(specs):
        run(["iptables", "-t", "filter", "-I", SG_CHAIN, "1", *spec], dry)
    if drop_blanket:
        remove_blanket_accepts(fixed, dry)


def apply_sg(fixed: str, ingress: list[SgRule], egress: list[SgRule], dry: bool) -> None:
    specs = policy_specs(fixed, ingress, egress)
    install_sg_specs(fixed, specs, dry, bool(ingress))
    if not dry:
        write_sg_state(fixed, specs)


def read_fip_record(name: str) -> Optional[tuple[str, str]]:
    """(floating, fixed) when `name` is one stored floating IP. Else None."""
    if not ID_RE.match(name):
        return None
    path = os.path.join(state_dir(), name)
    if not os.path.isfile(path):
        return None
    try:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(data, dict):
        return None
    floating = data.get("floating")
    fixed = data.get("fixed")
    if not isinstance(floating, str) or not isinstance(fixed, str):
        return None
    if not floating or not fixed:
        return None
    return floating, fixed


def sg_fixed_from_name(name: str) -> Optional[str]:
    """Guest address encoded in an `sg-<ip>` state file, or None."""
    prefix = "sg-"
    if not name.startswith(prefix):
        return None
    text = name[len(prefix) :]
    try:
        ip = ipaddress.ip_address(text)
    except ValueError:
        return None
    if not isinstance(ip, ipaddress.IPv4Address) or str(ip) != text:
        return None
    return text


def replay_sg(fixed: str) -> None:
    """Reinstall one stored group policy through the sg install path.

    The state file stays. `sg --clear` is what unlinks it.
    """
    specs = load_saved_specs(fixed)
    if not specs:
        return
    drop_blanket = any(comment_of(spec).endswith("-in-drop") for spec in specs)
    install_sg_specs(fixed, specs, False, drop_blanket, unlink=False)


def restore_plan() -> list[list[str]]:
    """Commands restore will run. Reads state and does not call the host."""
    root = state_dir()
    if not os.path.isdir(root):
        return []
    try:
        names = sorted(os.listdir(root))
    except OSError as exc:
        die(f"cannot read {root}: {exc}")
    plan: list[list[str]] = []
    for name in names:
        rec = read_fip_record(name)
        if rec is None:
            continue
        floating, fixed = rec
        plan.append(["ensure", "--id", name, "--floating", floating, "--fixed", fixed])
    for name in names:
        fixed = sg_fixed_from_name(name)
        if fixed is None or not load_saved_specs(fixed):
            continue
        plan.append(["sg", "--fixed", fixed])
    for bridge in read_bridges():
        plan.append(["metadata-on", "--bridge", bridge])
    return plan


def restore() -> None:
    """Replay stored NAT. Missing or empty state adds no rules."""
    plan = restore_plan()
    if not plan:
        print("pistack-fip: nothing to restore")
        return
    for cmd in plan:
        if cmd[0] == "ensure":
            ensure(cmd[2], cmd[4], cmd[6], False)
        elif cmd[0] == "sg":
            replay_sg(cmd[2])
        elif cmd[0] == "metadata-on":
            metadata_on(cmd[2])
        else:
            die(f"unknown restore action {cmd[0]}")


def sg_main(fixed_text: str, policy: Optional[str], clear: bool, dry: bool) -> None:
    fixed_ip = parse_ip(fixed_text, "fixed")
    check_fixed(fixed_ip)
    fixed = str(fixed_ip)
    if clear and policy is not None:
        die("pass only one of --policy and --clear")
    if clear:
        clear_sg(fixed, dry)
        return
    if policy is None:
        die("sg needs --policy or --clear")
    if policy == "-":
        raw = sys.stdin.read()
    else:
        try:
            with open(policy, encoding="utf-8") as fh:
                raw = fh.read()
        except OSError as exc:
            die(f"cannot read policy: {exc}")
    ingress, egress = parse_policy(raw)
    apply_sg(fixed, ingress, egress, dry)


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
        die(f"self-test: {what} exited {exc.code}: {buf.getvalue().strip()}")
    else:
        sys.stderr = held
        die(f"self-test: {what} was accepted")
    finally:
        sys.stderr = held


def _assert(cond: bool, what: str) -> None:
    if not cond:
        die(f"self-test: {what}")


def _stdout(fn) -> str:
    held = sys.stdout
    buf = io.StringIO()
    sys.stdout = buf
    try:
        fn()
    finally:
        sys.stdout = held
    return buf.getvalue()


def self_test() -> None:
    """Check CIDR policy, bridge names, and restore planning without the host.

    State goes to a temp directory via PISTACK_FIP_STATE. iptables, ip, and
    sysctl are not invoked.
    """
    saved_env = os.environ.get("PISTACK_FIP_STATE")
    td = tempfile.mkdtemp(prefix="pistack-fip-selftest-")
    os.environ["PISTACK_FIP_STATE"] = td
    real_run = subprocess.run

    def _no_host(*_args, **_kwargs):
        raise RuntimeError("self-test must not run host commands")

    subprocess.run = _no_host  # type: ignore[assignment]
    try:
        _run_self_test()
    finally:
        subprocess.run = real_run  # type: ignore[assignment]
        if saved_env is None:
            os.environ.pop("PISTACK_FIP_STATE", None)
        else:
            os.environ["PISTACK_FIP_STATE"] = saved_env
        shutil.rmtree(td, ignore_errors=True)
    print("pistack-fip: self-test ok")


def _run_self_test() -> None:
    text = _stdout(restore)
    _assert(text.strip() == "pistack-fip: nothing to restore", f"empty restore {text!r}")
    _assert(os.listdir(state_dir()) == [], "empty restore wrote state")
    check_fixed(parse_ip("10.88.0.187", "fixed"))
    _refuses(lambda: check_fixed(parse_ip("10.90.0.10", "fixed")), "10.90.0.10 before allow-list")
    _refuses(lambda: check_fixed(parse_ip("10.88.0.1", "fixed")), "10.88.0.1")
    _refuses(lambda: check_fixed(parse_ip("10.88.0.0", "fixed")), "10.88.0.0")
    _refuses(lambda: check_fixed(parse_ip("10.88.0.255", "fixed")), "10.88.0.255")
    for bad in (
        "10.66.66.0/24",
        "10.66.66.80/28",
        "10.66.66.1/32",
        "10.66.64.0/20",
        "10.90.0.0/19",
    ):
        _refuses(lambda cidr=bad: add_guest_cidr(cidr), f"refused {bad}")
    _assert(not os.path.exists(guest_cidr_path()), "rejected cidr was stored")
    add_guest_cidr("10.90.0.0/24")
    add_guest_cidr("10.90.0.0/24")
    check_fixed(parse_ip("10.90.0.10", "fixed"))
    check_fixed(parse_ip("10.88.0.187", "fixed"))
    _refuses(lambda: check_fixed(parse_ip("10.90.0.0", "fixed")), "10.90.0.0")
    _refuses(lambda: check_fixed(parse_ip("10.90.0.1", "fixed")), "10.90.0.1")
    _refuses(lambda: check_fixed(parse_ip("10.90.0.255", "fixed")), "10.90.0.255")
    # /23 gateway is network+1 (10.91.0.1), not every address that ends in .1.
    add_guest_cidr("10.91.0.0/23")
    check_fixed(parse_ip("10.91.1.1", "fixed"))
    _refuses(lambda: check_fixed(parse_ip("10.91.0.1", "fixed")), "10.91.0.1")
    _refuses(lambda: check_fixed(parse_ip("10.91.1.255", "fixed")), "10.91.1.255")
    add_guest_cidr("10.100.0.0/20")
    _refuses(lambda: add_guest_cidr("10.100.0.0/19"), "prefix 19")
    with open(guest_cidr_path(), encoding="utf-8") as fh:
        lines = fh.read().splitlines()
    _assert(lines.count("10.90.0.0/24") == 1, f"idempotent store {lines}")
    _assert("10.66.66.0/24" not in lines, f"fleet cidr stored {lines}")
    _assert("10.66.66.80/28" not in lines, f"pool cidr stored {lines}")
    # Adding the builtin must not drop it from the allow-list.
    add_guest_cidr("10.88.0.0/24")
    check_fixed(parse_ip("10.88.0.187", "fixed"))
    _refuses(lambda: check_bridge("eth0"), "eth0")
    _refuses(lambda: check_bridge("wg-fleet"), "wg-fleet")
    _refuses(lambda: check_bridge("pistack0/eth0"), "slash")
    _refuses(lambda: check_bridge("pi.stack"), "dot")
    check_bridge("pistack0")
    check_bridge("pi01234567")
    check_bridge("pi0123456789ab")
    _refuses(lambda: check_bridge("pi0123456"), "short bridge")
    _refuses(lambda: check_bridge("pi0123456789abc"), "long bridge")
    dnat = metadata_dnat_spec("pi01a10318a507")
    _assert(dnat[dnat.index("-j") + 1] == "DNAT", "metadata dnat target")
    _assert(dnat[-1] == "127.0.0.1:18880", "metadata dnat destination")
    _assert("REDIRECT" not in dnat, "metadata dnat is not a redirect")
    _assert("wg-fleet" not in dnat, "metadata dnat stays off wg-fleet")
    _assert(dnat[1] == "pi01a10318a507", "metadata dnat is one bridge")
    inbound = metadata_input_spec("pistack0")
    _assert(inbound[1] == "pistack0", "metadata input is one bridge")
    _assert("18880" in inbound and inbound[-1] == "ACCEPT", "metadata input port")
    _assert("0.0.0.0/0" not in inbound, "metadata input is not wide open")
    sysctl = metadata_sysctl("pi01a10318a507")
    _assert(sysctl[-1] == "net.ipv4.conf.pi01a10318a507.route_localnet=1", "per-bridge route_localnet")
    _assert("all.route_localnet" not in " ".join(sysctl), "route_localnet is not global")
    redirects = metadata_redirect_specs("pistack0")
    _assert(len(redirects) == 2, "both old redirect forms")
    _assert(all(spec[spec.index("-j") + 1] == "REDIRECT" for spec in redirects), "old redirect target")
    # One real fip, one bridge, plus files that are not fip or sg state.
    fip_id = "fip-01234567-89ab-cdef-0123-456789abcdef"
    with open(os.path.join(state_dir(), fip_id), "w", encoding="utf-8") as fh:
        json.dump({"floating": "10.66.66.82", "fixed": "10.88.0.187"}, fh)
    with open(
        os.path.join(state_dir(), "fip-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
        "w",
        encoding="utf-8",
    ) as fh:
        fh.write("not-json\n")
    with open(bridges_path(), "w", encoding="utf-8") as fh:
        fh.write("pistack0\n")
    with open(sg_state_path("10.88.0.187"), "w", encoding="utf-8") as fh:
        json.dump(
            {"fixed": "10.88.0.187", "rules": [["-d", "10.88.0.187/32", "-j", "ACCEPT"]]},
            fh,
        )
    with open(sg_state_path("10.90.0.2"), "w", encoding="utf-8") as fh:
        json.dump({"fixed": "10.90.0.2", "rules": []}, fh)
    with open(guest_cidr_path(), encoding="utf-8") as fh:
        cidrs_before = fh.read()
    plan = restore_plan()
    _assert(
        plan
        == [
            [
                "ensure",
                "--id",
                fip_id,
                "--floating",
                "10.66.66.82",
                "--fixed",
                "10.88.0.187",
            ],
            ["sg", "--fixed", "10.88.0.187"],
            ["metadata-on", "--bridge", "pistack0"],
        ],
        f"restore plan {plan}",
    )
    with open(guest_cidr_path(), encoding="utf-8") as fh:
        _assert(fh.read() == cidrs_before, "restore plan rewrote guest-cidrs")
    _refuses(lambda: metadata_on("eth0"), "metadata-on eth0")
    with open(bridges_path(), encoding="utf-8") as fh:
        _assert(fh.read().splitlines() == ["pistack0"], "rejected bridge was stored")
    # Host half stays stubbed. A second metadata-on must not add a line.
    previous_apply = globals()["_apply_metadata"]

    def _quiet(_name: str) -> None:
        return

    globals()["_apply_metadata"] = _quiet
    try:
        metadata_on("pistack0")
        metadata_on("pistack0")
        metadata_on("pi01234567")
        metadata_on("pi01234567")
        with open(bridges_path(), encoding="utf-8") as fh:
            lines = fh.read().splitlines()
        _assert(lines == ["pistack0", "pi01234567"], f"duplicate metadata-on record {lines}")
    finally:
        globals()["_apply_metadata"] = previous_apply
    saved_state = os.environ["PISTACK_FIP_STATE"]
    missing = os.path.join(saved_state, "missing")
    os.environ["PISTACK_FIP_STATE"] = missing
    try:
        text = _stdout(restore)
        _assert(text.strip() == "pistack-fip: nothing to restore", f"missing dir {text!r}")
        _assert(not os.path.exists(missing), "missing dir was created")
    finally:
        os.environ["PISTACK_FIP_STATE"] = saved_state
    empty = os.path.join(saved_state, "empty")
    os.makedirs(empty)
    os.environ["PISTACK_FIP_STATE"] = empty
    try:
        with open(os.path.join(empty, "bridges"), "w", encoding="utf-8") as fh:
            fh.write("\n")
        with open(os.path.join(empty, "guest-cidrs"), "w", encoding="utf-8") as fh:
            fh.write("\n")
        text = _stdout(restore)
        _assert(text.strip() == "pistack-fip: nothing to restore", f"empty files {text!r}")
    finally:
        os.environ["PISTACK_FIP_STATE"] = saved_state


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="check CIDR, bridge, and restore planning without touching the host",
    )
    sub = parser.add_subparsers(dest="cmd")
    ensure_p = sub.add_parser("ensure")
    ensure_p.add_argument("--id", required=True)
    ensure_p.add_argument("--floating", required=True)
    ensure_p.add_argument("--fixed", required=True)
    ensure_p.add_argument("--dry-run", action="store_true")
    delete_p = sub.add_parser("delete")
    delete_p.add_argument("--id", required=True)
    delete_p.add_argument("--dry-run", action="store_true")
    sg_p = sub.add_parser("sg")
    sg_p.add_argument("--fixed", required=True)
    sg_p.add_argument("--policy", help="JSON file, or - to read stdin")
    sg_p.add_argument("--clear", action="store_true")
    sg_p.add_argument("--dry-run", action="store_true")
    gc_p = sub.add_parser("guest-cidr")
    gc_sub = gc_p.add_subparsers(dest="gc_cmd", required=True)
    gc_add = gc_sub.add_parser("add")
    gc_add.add_argument("--cidr", required=True)
    md_p = sub.add_parser("metadata-on")
    md_p.add_argument("--bridge", required=True)
    sub.add_parser("restore")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if args.cmd == "sg":
        sg_main(args.fixed, args.policy, args.clear, args.dry_run)
        return
    if args.cmd == "guest-cidr":
        add_guest_cidr(args.cidr)
        return
    if args.cmd == "metadata-on":
        metadata_on(args.bridge)
        return
    if args.cmd == "restore":
        restore()
        return
    if args.cmd is None:
        parser.error("a command is required")
    fip_id = check_id(args.id)
    if args.cmd == "ensure":
        ensure(fip_id, args.floating, args.fixed, args.dry_run)
    else:
        delete(fip_id, args.dry_run)


if __name__ == "__main__":
    main()
