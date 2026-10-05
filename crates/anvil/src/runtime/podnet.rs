// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Join a youki pod to the bridges its ports already name.
//!
//! Virtual machines attach a tap with `ip link set <tap> master <bridge>`.
//! A pod uses the same bridge, computed by [`crate::runtime::libvirt::port_bridge`],
//! and a veth instead of a tap. The host end stays on the bridge. The other
//! end moves into a named network namespace that youki joins.
//!
//! Names are short and checked before `ip` runs: `pn` plus 12 hex digits of
//! the machine id for the namespace, `vh` / `vp` plus 12 hex digits of the
//! MAC for the veth ends, and `eth0`, `eth1`, ... inside the namespace.
//! `$PISTACK_IP_BIN`, when it is set, replaces `ip` (tests point this at a
//! fake). A root agent runs that binary directly. Sudo mode runs
//! `sudo -n <ip> ...`. A rootless agent cannot, and [`prepare`] says so
//! without calling `ip`.

use super::{MachineRequest, NicAttachment};
use pistack_types::{ApiError, ApiResult, Id};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const NETNS_DIR: &str = "/var/run/netns";
const TOOL_WAIT: Duration = Duration::from_secs(10);
const IP_BIN_ENV: &str = "PISTACK_IP_BIN";
/// What a rootless agent reports instead of the recorded address.
pub(crate) const ROOTLESS_NETWORK_MESSAGE: &str = "pod has no network: a rootless agent cannot attach the recorded address. Run the agent as root or set PISTACK_YOUKI_SUDO=1";

/// How `ip` may be started. Rootless never reaches the binary.
#[derive(Clone, Debug)]
pub(crate) enum IpPrivilege {
    Direct,
    Sudo(PathBuf),
    Rootless,
}

/// Namespace and veths created for one pod.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PodLink {
    pub netns: String,
    pub path: PathBuf,
    pub veths: Vec<String>,
}

/// What [`prepare`] decided. `None` keeps the sandbox network namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PodNetwork {
    None,
    Linked(PodLink),
    Refused(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Attachment {
    host: String,
    peer: String,
    guest: String,
    bridge: String,
    mac: String,
    cidr: String,
    gateway: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    pub netns: String,
    pub path: PathBuf,
    attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct NetRecord {
    netns: String,
    veths: Vec<String>,
}

/// True when `nic` carries a port, not only an address string.
pub(crate) fn is_attachment(nic: &NicAttachment) -> bool {
    !nic.mac.is_empty()
        || !nic.network_id.is_empty()
        || nic.prefix != 0
        || nic.segment_id.is_some()
        || !nic.gateway.is_empty()
}

/// `pn` plus 12 lowercase hex digits. Anything else is refused.
pub(crate) fn netns_name_ok(name: &str) -> bool {
    prefixed_hex(name, "pn")
}

/// `vh` or `vp` plus 12 lowercase hex digits.
pub(crate) fn veth_name_ok(name: &str) -> bool {
    prefixed_hex(name, "vh") || prefixed_hex(name, "vp")
}

/// `eth0`, `eth1`, ... `eth15`.
pub(crate) fn guest_name_ok(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("eth") else {
        return false;
    };
    let Ok(index) = rest.parse::<u8>() else {
        return false;
    };
    index < 16 && rest.len() == if index < 10 { 1 } else { 2 } && name.len() <= 5
}

fn prefixed_hex(name: &str, prefix: &str) -> bool {
    let Some(hex) = name.strip_prefix(prefix) else {
        return false;
    };
    name.len() == prefix.len() + 12
        && hex.len() == 12
        && hex
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Namespace name for `id`. The kind prefix is not part of the hex.
pub(crate) fn netns_name(id: &Id) -> ApiResult<String> {
    let hex: String = id.as_uuid_simple().chars().take(12).collect();
    let name = format!("pn{hex}");
    if !netns_name_ok(&name) {
        return Err(ApiError::bad_request(
            "youki runtime: network namespace name is not allowed",
        ));
    }
    Ok(name)
}

pub(crate) fn plan(req: &MachineRequest) -> ApiResult<Option<Plan>> {
    if !req.nics.iter().any(is_attachment) {
        return Ok(None);
    }
    if req.nics.len() > 16 {
        return Err(ApiError::bad_request(
            "youki runtime: too many network attachments",
        ));
    }
    let netns = netns_name(&req.id)?;
    let mut attachments = Vec::with_capacity(req.nics.len());
    for (index, nic) in req.nics.iter().enumerate() {
        attachments.push(attachment(index, nic)?);
    }
    let path = Path::new(NETNS_DIR).join(&netns);
    if !path.is_absolute() || path.components().any(|c| c.as_os_str() == "..") {
        return Err(ApiError::bad_request(
            "youki runtime: network namespace path is not allowed",
        ));
    }
    let plan = Plan {
        netns,
        path,
        attachments,
    };
    // Refuse before `ip` if any token would not match the name rules.
    for cmd in setup_argv(&plan) {
        for token in &cmd {
            if !token_ok(token) {
                return Err(ApiError::bad_request(
                    "youki runtime: network name is not allowed",
                ));
            }
        }
    }
    Ok(Some(plan))
}

fn attachment(index: usize, nic: &NicAttachment) -> ApiResult<Attachment> {
    let bridge = crate::runtime::libvirt::port_bridge(nic)?;
    if !bridge_name_ok(&bridge) {
        return Err(ApiError::bad_request(
            "youki runtime: port bridge is not allowed",
        ));
    }
    let mac = canonical_mac(&nic.mac)?;
    let host = format!("vh{}", mac_hex(&mac));
    let peer = format!("vp{}", mac_hex(&mac));
    if !veth_name_ok(&host) || !veth_name_ok(&peer) {
        return Err(ApiError::bad_request("nic mac is not an address"));
    }
    let guest = format!("eth{index}");
    if !guest_name_ok(&guest) {
        return Err(ApiError::bad_request(
            "youki runtime: guest interface name is not allowed",
        ));
    }
    let addr: Ipv4Addr = nic
        .address
        .parse()
        .map_err(|_| ApiError::bad_request("nic address must be ipv4"))?;
    if nic.prefix == 0 || nic.prefix > 30 {
        return Err(ApiError::bad_request("nic prefix must be 1-30"));
    }
    let cidr = format!("{addr}/{}", nic.prefix);
    let gateway = if nic.gateway.is_empty() {
        String::new()
    } else {
        let gw: Ipv4Addr = nic
            .gateway
            .parse()
            .map_err(|_| ApiError::bad_request("nic gateway must be ipv4"))?;
        gw.to_string()
    };
    Ok(Attachment {
        host,
        peer,
        guest,
        bridge,
        mac,
        cidr,
        gateway,
    })
}

/// `ip` argv for a fresh namespace, including the leftover delete that
/// runs first. Delete commands tolerate a missing device.
pub(crate) fn setup_argv(plan: &Plan) -> Vec<Vec<String>> {
    let mut cmds = teardown_argv(plan);
    cmds.push(vec!["netns".into(), "add".into(), plan.netns.clone()]);
    let mut route = None;
    for att in &plan.attachments {
        cmds.push(vec![
            "link".into(),
            "add".into(),
            att.host.clone(),
            "type".into(),
            "veth".into(),
            "peer".into(),
            "name".into(),
            att.peer.clone(),
        ]);
        cmds.push(vec![
            "link".into(),
            "set".into(),
            att.host.clone(),
            "master".into(),
            att.bridge.clone(),
        ]);
        cmds.push(vec![
            "link".into(),
            "set".into(),
            att.host.clone(),
            "up".into(),
        ]);
        cmds.push(vec![
            "link".into(),
            "set".into(),
            att.peer.clone(),
            "netns".into(),
            plan.netns.clone(),
        ]);
        cmds.push(vec![
            "-n".into(),
            plan.netns.clone(),
            "link".into(),
            "set".into(),
            att.peer.clone(),
            "name".into(),
            att.guest.clone(),
        ]);
        cmds.push(vec![
            "-n".into(),
            plan.netns.clone(),
            "link".into(),
            "set".into(),
            att.guest.clone(),
            "address".into(),
            att.mac.clone(),
        ]);
        cmds.push(vec![
            "-n".into(),
            plan.netns.clone(),
            "addr".into(),
            "add".into(),
            att.cidr.clone(),
            "dev".into(),
            att.guest.clone(),
        ]);
        if att.guest == "eth0" {
            cmds.push(vec![
                "-n".into(),
                plan.netns.clone(),
                "link".into(),
                "set".into(),
                "lo".into(),
                "up".into(),
            ]);
        }
        cmds.push(vec![
            "-n".into(),
            plan.netns.clone(),
            "link".into(),
            "set".into(),
            att.guest.clone(),
            "up".into(),
        ]);
        if route.is_none() && !att.gateway.is_empty() {
            route = Some(att.gateway.clone());
        }
    }
    if let Some(gw) = route {
        cmds.push(vec![
            "-n".into(),
            plan.netns.clone(),
            "route".into(),
            "add".into(),
            "default".into(),
            "via".into(),
            gw,
        ]);
    }
    cmds
}

/// `ip` argv that removes the veth ends and the namespace.
pub(crate) fn teardown_argv(plan: &Plan) -> Vec<Vec<String>> {
    teardown_names(&plan.netns, &plan.veths())
}

fn teardown_names(netns: &str, veths: &[String]) -> Vec<Vec<String>> {
    let mut cmds = Vec::new();
    for host in veths {
        cmds.push(vec!["link".into(), "del".into(), host.clone()]);
        if let Some(peer) = peer_of(host) {
            cmds.push(vec!["link".into(), "del".into(), peer]);
        }
    }
    if !netns.is_empty() {
        cmds.push(vec!["netns".into(), "del".into(), netns.to_owned()]);
    }
    cmds
}

impl Plan {
    fn veths(&self) -> Vec<String> {
        self.attachments.iter().map(|a| a.host.clone()).collect()
    }

    fn link(&self) -> PodLink {
        PodLink {
            netns: self.netns.clone(),
            path: self.path.clone(),
            veths: self.veths(),
        }
    }
}

fn peer_of(host: &str) -> Option<String> {
    let hex = host.strip_prefix("vh")?;
    let peer = format!("vp{hex}");
    veth_name_ok(&peer).then_some(peer)
}

/// Create the namespace and veths. A rootless agent returns [`PodNetwork::Refused`]
/// and does not run `ip`. No attachment returns [`PodNetwork::None`].
pub(crate) async fn prepare(
    req: &MachineRequest,
    privilege: &IpPrivilege,
    bundle: &Path,
) -> ApiResult<PodNetwork> {
    let Some(plan) = plan(req)? else {
        return Ok(PodNetwork::None);
    };
    if matches!(privilege, IpPrivilege::Rootless) {
        return Ok(PodNetwork::Refused(ROOTLESS_NETWORK_MESSAGE.to_owned()));
    }
    write_record(bundle, &plan.netns, &plan.veths())?;
    if let Err(err) = apply(privilege, &setup_argv(&plan)).await {
        let _ = release(privilege, &plan.netns, &plan.veths()).await;
        return Err(err);
    }
    Ok(PodNetwork::Linked(plan.link()))
}

/// Delete the veths and the namespace. Missing devices are success.
/// Empty names do not run `ip`. A name that fails the regex is skipped.
pub(crate) async fn release(
    privilege: &IpPrivilege,
    netns: &str,
    veths: &[String],
) -> ApiResult<()> {
    if matches!(privilege, IpPrivilege::Rootless) {
        return Ok(());
    }
    let netns = if netns_name_ok(netns) { netns } else { "" };
    let veths: Vec<String> = veths.iter().filter(|n| veth_name_ok(n)).cloned().collect();
    if netns.is_empty() && veths.is_empty() {
        return Ok(());
    }
    let mut first = None;
    for cmd in teardown_names(netns, &veths) {
        if let Err(err) = run_ip(privilege, &cmd).await {
            if !already_gone(&err.message) && first.is_none() {
                first = Some(err);
            }
        }
    }
    match first {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

pub(crate) fn read_record(bundle: &Path) -> Option<(String, Vec<String>)> {
    let bytes = std::fs::read(bundle.join("net.json")).ok()?;
    let rec: NetRecord = serde_json::from_slice(&bytes).ok()?;
    let netns = if netns_name_ok(&rec.netns) {
        rec.netns
    } else {
        String::new()
    };
    let veths: Vec<String> = rec.veths.into_iter().filter(|n| veth_name_ok(n)).collect();
    if netns.is_empty() && veths.is_empty() {
        None
    } else {
        Some((netns, veths))
    }
}

fn write_record(bundle: &Path, netns: &str, veths: &[String]) -> ApiResult<()> {
    std::fs::create_dir_all(bundle).map_err(|err| {
        ApiError::unavailable(format!(
            "youki runtime: creating {}: {err}",
            bundle.display()
        ))
    })?;
    let rec = NetRecord {
        netns: netns.to_owned(),
        veths: veths.to_vec(),
    };
    let text = serde_json::to_vec_pretty(&rec)
        .map_err(|err| ApiError::internal(format!("youki runtime: encoding net record: {err}")))?;
    let path = bundle.join("net.json");
    std::fs::write(&path, text).map_err(|err| {
        ApiError::unavailable(format!("youki runtime: writing {}: {err}", path.display()))
    })
}

async fn apply(privilege: &IpPrivilege, cmds: &[Vec<String>]) -> ApiResult<()> {
    for cmd in cmds {
        let result = run_ip(privilege, cmd).await;
        if is_delete(cmd) {
            if let Err(err) = result {
                if !already_gone(&err.message) {
                    return Err(err);
                }
            }
        } else if let Err(err) = result {
            return Err(err);
        }
    }
    Ok(())
}

fn is_delete(cmd: &[String]) -> bool {
    matches!(
        cmd,
        [op, action, ..] if (op == "link" || op == "netns") && action == "del"
    )
}

fn bridge_name_ok(name: &str) -> bool {
    if name == "pistack0" {
        return true;
    }
    let Some(hex) = name.strip_prefix("po").or_else(|| name.strip_prefix("pi")) else {
        return false;
    };
    name.len() == 14 && hex.len() == 12 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn canonical_mac(mac: &str) -> ApiResult<String> {
    let parts: Vec<&str> = mac.split(':').collect();
    if parts.len() != 6
        || !parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(ApiError::bad_request("nic mac is not an address"));
    }
    Ok(parts
        .iter()
        .map(|p| p.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(":"))
}

fn mac_hex(mac: &str) -> String {
    mac.chars().filter(|c| *c != ':').collect()
}

fn token_ok(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 64
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '/' | '-' | '_'))
}

fn ip_bin() -> ApiResult<PathBuf> {
    if let Some(value) = std::env::var_os(IP_BIN_ENV) {
        if !value.is_empty() {
            let path = PathBuf::from(&value);
            if is_executable(&path) {
                return Ok(path);
            }
            return Err(ApiError::unavailable(
                "youki runtime: ip helper is not available",
            ));
        }
    }
    crate::facts::which("ip")
        .ok_or_else(|| ApiError::unavailable("youki runtime: ip is not available"))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

async fn run_ip(privilege: &IpPrivilege, args: &[String]) -> ApiResult<()> {
    let ip = ip_bin()?;
    let (program, elevate) = match privilege {
        IpPrivilege::Rootless => {
            return Err(ApiError::unavailable(ROOTLESS_NETWORK_MESSAGE));
        }
        IpPrivilege::Direct => (ip.clone(), None),
        IpPrivilege::Sudo(sudo) => (sudo.clone(), Some(ip)),
    };
    let mut cmd = Command::new(&program);
    if let Some(ip) = &elevate {
        cmd.arg("-n").arg(ip);
    }
    cmd.args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let name = program.file_name().and_then(|s| s.to_str()).unwrap_or("ip");
    match tokio::time::timeout(TOOL_WAIT, cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let msg = trim_msg(err.trim());
            Err(ApiError::unavailable(if msg.is_empty() {
                format!("youki runtime: {name} failed")
            } else {
                format!("youki runtime: {name} failed: {msg}")
            }))
        }
        Ok(Err(err)) => Err(ApiError::unavailable(format!(
            "youki runtime: cannot run {name}: {err}"
        ))),
        Err(_) => Err(ApiError::unavailable(format!(
            "youki runtime: {name} timed out"
        ))),
    }
}

fn trim_msg(msg: &str) -> String {
    msg.chars().take(500).collect()
}

fn already_gone(msg: &str) -> bool {
    let msg = msg.to_ascii_lowercase();
    msg.contains("cannot find device")
        || msg.contains("no such file")
        || msg.contains("does not exist")
        || msg.contains("no such device")
        || msg.contains("not found")
}

trait UuidSimple {
    fn as_uuid_simple(&self) -> String;
}

impl UuidSimple for Id {
    fn as_uuid_simple(&self) -> String {
        self.uuid.simple().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_types::kind;

    fn machine_id() -> Id {
        Id::parse(kind::MACHINE, "mach-018f3a2b-1c4d-7e5f-8a6b-0c1d2e3f4a5b").unwrap()
    }

    fn nic(address: &str, mac: &str, gateway: &str, segment: Option<u32>) -> NicAttachment {
        NicAttachment {
            network_id: "net-01a102de-3f33-73bf-86f7-57fe3b47f1ea".into(),
            mac: mac.into(),
            address: address.into(),
            prefix: 24,
            gateway: gateway.into(),
            routed: !gateway.is_empty(),
            segment_id: segment,
        }
    }

    fn request(nics: Vec<NicAttachment>) -> MachineRequest {
        MachineRequest {
            id: machine_id(),
            name: "web".into(),
            kind: super::super::MachineKind::Container,
            image: "/img".into(),
            resources: Default::default(),
            networks: vec!["labnet".into()],
            nics,
            volumes: vec![],
            user_data: None,
            ssh_keys: vec![],
            overlay: None,
            bmc_address: None,
            bmc_user: None,
        }
    }

    #[test]
    fn names_match_a_strict_pattern() {
        assert_eq!(netns_name(&machine_id()).unwrap(), "pn018f3a2b1c4d");
        assert!(netns_name_ok("pn018f3a2b1c4d"));
        assert!(veth_name_ok("vh525400aabbcc"));
        assert!(veth_name_ok("vp525400aabbcc"));
        assert!(guest_name_ok("eth0"));
        assert!(guest_name_ok("eth15"));
        for bad in [
            "pn018F3A2B1C4D",
            "pn018f3a2b1c4",
            "pn018f3a2b1c4d0",
            "ct018f3a2b1c4d",
            "pn018f3a2b1c4d;ip",
            "../pn018f3a2b1c4d",
            "",
            "pn018f3a2b1c4d/x",
        ] {
            assert!(!netns_name_ok(bad), "{bad}");
        }
        for bad in [
            "vh525400AABBCC",
            "vh525400aabb",
            "tap525400aabbcc",
            "vh 525400aabbcc",
        ] {
            assert!(!veth_name_ok(bad), "{bad}");
        }
        for bad in ["eth", "eth0extra", "ens0", "eth16", "eth01"] {
            assert!(!guest_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn setup_argv_creates_the_namespace_veth_bridge_address_and_route() {
        let req = request(vec![
            nic("10.90.0.10", "52:54:00:AA:BB:CC", "10.90.0.1", None),
            nic("10.90.0.11", "52:54:00:11:22:33", "", Some(4)),
        ]);
        let plan = plan(&req).unwrap().unwrap();
        assert_eq!(plan.netns, "pn018f3a2b1c4d");
        assert_eq!(plan.path, PathBuf::from("/var/run/netns/pn018f3a2b1c4d"));
        let cmds = setup_argv(&plan);
        assert_eq!(
            cmds[0],
            vec!["link", "del", "vh525400aabbcc"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        );
        assert!(cmds
            .iter()
            .any(|c| c == &owned(&["link", "del", "vp525400aabbcc"])));
        assert!(cmds
            .iter()
            .any(|c| c == &owned(&["link", "del", "vh525400112233"])));
        assert!(cmds
            .iter()
            .any(|c| c == &owned(&["netns", "del", "pn018f3a2b1c4d"])));
        assert!(cmds
            .iter()
            .any(|c| c == &owned(&["netns", "add", "pn018f3a2b1c4d"])));
        assert!(cmds.iter().any(|c| {
            c == &owned(&[
                "link",
                "add",
                "vh525400aabbcc",
                "type",
                "veth",
                "peer",
                "name",
                "vp525400aabbcc",
            ])
        }));
        assert!(cmds
            .iter()
            .any(|c| c == &owned(&["link", "set", "vh525400aabbcc", "master", "pi01a102de3f33"])));
        assert!(cmds
            .iter()
            .any(|c| c == &owned(&["link", "set", "vh525400112233", "master", "po01a102de3f33"])));
        assert!(!cmds
            .iter()
            .any(|c| c.windows(2).any(|w| w == ["master", "pistack0"])));
        assert!(cmds.iter().any(|c| {
            c == &owned(&[
                "-n",
                "pn018f3a2b1c4d",
                "link",
                "set",
                "vp525400aabbcc",
                "name",
                "eth0",
            ])
        }));
        assert!(cmds.iter().any(|c| {
            c == &owned(&[
                "-n",
                "pn018f3a2b1c4d",
                "link",
                "set",
                "eth0",
                "address",
                "52:54:00:aa:bb:cc",
            ])
        }));
        assert!(cmds.iter().any(|c| {
            c == &owned(&[
                "-n",
                "pn018f3a2b1c4d",
                "addr",
                "add",
                "10.90.0.10/24",
                "dev",
                "eth0",
            ])
        }));
        assert!(cmds
            .iter()
            .any(|c| { c == &owned(&["-n", "pn018f3a2b1c4d", "link", "set", "lo", "up"]) }));
        assert!(cmds
            .iter()
            .any(|c| { c == &owned(&["-n", "pn018f3a2b1c4d", "link", "set", "eth0", "up"]) }));
        assert!(cmds.iter().any(|c| {
            c == &owned(&[
                "-n",
                "pn018f3a2b1c4d",
                "link",
                "set",
                "vp525400112233",
                "name",
                "eth1",
            ])
        }));
        assert!(cmds.iter().any(|c| {
            c == &owned(&[
                "-n",
                "pn018f3a2b1c4d",
                "route",
                "add",
                "default",
                "via",
                "10.90.0.1",
            ])
        }));
        assert_eq!(
            cmds.iter()
                .filter(|c| c.windows(3).any(|w| w == ["route", "add", "default"]))
                .count(),
            1
        );
        let tear = teardown_argv(&plan);
        assert!(tear
            .iter()
            .any(|c| c == &owned(&["link", "del", "vh525400aabbcc"])));
        assert!(tear
            .iter()
            .any(|c| c == &owned(&["netns", "del", "pn018f3a2b1c4d"])));
        assert!(!tear.iter().any(|c| c.iter().any(|a| a == "add")));
    }

    #[test]
    fn a_bad_mac_or_a_bare_address_does_not_make_a_plan() {
        let bad = request(vec![nic("10.90.0.10", "not-a-mac", "10.90.0.1", None)]);
        let err = plan(&bad).unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::BadRequest);
        assert!(err.message.contains("mac"), "{err}");

        let bare = request(vec![NicAttachment {
            address: "10.1.0.5".into(),
            ..NicAttachment::default()
        }]);
        assert!(plan(&bare).unwrap().is_none());
        let quiet = request(vec![]);
        assert!(plan(&quiet).unwrap().is_none());
    }

    #[tokio::test]
    async fn rootless_prepare_refuses_without_reporting_success_as_linked() {
        let req = request(vec![nic(
            "10.90.0.10",
            "52:54:00:aa:bb:cc",
            "10.90.0.1",
            None,
        )]);
        let tmp = std::env::temp_dir().join(format!(
            "pistack-podnet-rootless-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let got = prepare(&req, &IpPrivilege::Rootless, &tmp).await.unwrap();
        match got {
            PodNetwork::Refused(message) => {
                assert!(message.contains("pod has no network"), "{message}");
                assert!(message.contains("PISTACK_YOUKI_SUDO"), "{message}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(read_record(&tmp).is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    fn owned(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_owned()).collect()
    }
}
