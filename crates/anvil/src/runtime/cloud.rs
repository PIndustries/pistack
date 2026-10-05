// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Cloud Hypervisor machine runtime.
//!
//! Selected when the host capability `cloud-hypervisor` is present and
//! neither `kvm` nor `libvirt` is. libvirt remains the runtime for those.
//! The binary is `$PISTACK_CLOUD_HYPERVISOR_BIN` when that is set, otherwise
//! `cloud-hypervisor` on `PATH`. State lives under `$PISTACK_CLOUD_STATE`
//! (default `/var/lib/pistack/cloud`). A missing binary is unavailable.
//! `user_data` and ssh keys are not logged and are not placed on the argv.
//! The process is recorded only after it is still running.
//!
//! Each port is a tap on the bridge libvirt would use: the `po` overlay
//! bridge when `segment_id` is set, otherwise the host NAT bridge. The guest
//! is started with `--net tap=<tap>,mac=<mac>`. Taps are removed on stop and
//! delete. Hot plug and unplug call `ch-remote add-net` and `remove-device`
//! when that binary is available. `$PISTACK_IP_BIN` and `$PISTACK_CH_REMOTE`
//! override those helpers. This runtime does not create bridges.

use super::{MachineRequest, MachineRuntime, NicAttachment, PowerState, RunningMachine};
use async_trait::async_trait;
use pistack_rpc::proto::PortPlug;
use pistack_types::{kind, ApiError, ApiResult, Id};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const DEFAULT_STATE: &str = "/var/lib/pistack/cloud";
const PROBE: Duration = Duration::from_millis(500);
const TOOL_WAIT: Duration = Duration::from_secs(10);

/// Host-side Cloud Hypervisor runtime. One directory per machine holds
/// `state.json`. The API socket lives in the temp directory.
#[derive(Debug, Default)]
pub struct CloudRuntime;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Saved {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    addresses: Vec<String>,
    #[serde(default)]
    power: PowerState,
    #[serde(default)]
    pid: i32,
    #[serde(default)]
    socket: String,
    /// Taps this machine owns. Empty on a process that was started before
    /// ports were wired, and empty again after stop removes them.
    #[serde(default)]
    nets: Vec<SavedNet>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SavedNet {
    mac: String,
    tap: String,
    bridge: String,
    id: String,
}

struct PortTap {
    mac: String,
    tap: String,
    bridge: String,
    id: String,
}

#[async_trait]
impl MachineRuntime for CloudRuntime {
    fn name(&self) -> &'static str {
        "cloud-hypervisor"
    }

    async fn start(&self, req: &MachineRequest) -> ApiResult<RunningMachine> {
        let _guard = lock().lock().await;
        let bin = resolve_bin()?;
        if protected_name(&req.name) {
            return Err(host_guard());
        }
        let id = req.id.clone();
        let dir = machine_dir(&id)?;
        let prior = read_state(&dir).await?;
        if let Some(saved) = prior.as_ref() {
            if protected_name(&saved.name) {
                return Err(host_guard());
            }
            if saved.power == PowerState::On && pid_alive(saved.pid) && parse_id(&saved.id)? == id {
                return Ok(RunningMachine {
                    id,
                    addresses: nic_addresses(req),
                    power: PowerState::On,
                });
            }
            if pid_alive(saved.pid) {
                stop_pid(saved.pid).await;
            }
            let (left, err) = delete_taps(&saved.nets).await;
            if let Some(err) = err {
                let mut failed = saved.clone();
                failed.nets = left;
                failed.power = PowerState::Off;
                failed.pid = 0;
                write_state(&dir, &failed).await?;
                return Err(err);
            }
        }
        if req.resources.vcpus == 0 || req.resources.memory_mib == 0 {
            return Err(ApiError::unavailable(
                "cloud-hypervisor runtime: machine resources are not set",
            ));
        }
        let disk = validate_disk(&req.image)?;
        let ports = plan_ports(&req.nics)?;
        if let Some(plan) = &req.overlay {
            crate::overlay::apply_plan(plan).await?;
        }
        let socket = socket_path(&id)?;
        let _ = std::fs::remove_file(&socket);
        tokio::fs::create_dir_all(&dir).await.map_err(|e| {
            ApiError::unavailable(format!(
                "cloud-hypervisor runtime: creating {}: {e}",
                dir.display()
            ))
        })?;

        let mut args: Vec<String> = Vec::new();
        args.push("--api-socket".into());
        args.push(socket.to_string_lossy().into_owned());
        args.push("--disk".into());
        args.push(format!("path={}", disk.display()));
        args.push("--cpus".into());
        args.push(format!("boot={}", req.resources.vcpus));
        args.push("--memory".into());
        args.push(format!("size={}M", req.resources.memory_mib));
        args.push("--console".into());
        args.push("off".into());
        args.push("--serial".into());
        args.push("off".into());
        if let Some(kernel) = kernel_file() {
            args.push("--kernel".into());
            args.push(kernel.to_string_lossy().into_owned());
        }
        for port in &ports {
            args.push("--net".into());
            args.push(net_arg(&port.tap, &port.mac, &port.id));
        }

        if let Err(e) = create_ports(&ports).await {
            if prior.is_none() {
                let _ = tokio::fs::remove_dir_all(&dir).await;
            }
            return Err(e);
        }

        let mut cmd = Command::new(&bin);
        cmd.args(&args)
            .kill_on_drop(false)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                release_taps(&ports).await;
                let _ = std::fs::remove_file(&socket);
                if prior.is_none() {
                    let _ = tokio::fs::remove_dir_all(&dir).await;
                }
                return Err(ApiError::unavailable(format!(
                    "cloud-hypervisor runtime: cannot run {}: {e}",
                    bin.display()
                )));
            }
        };
        let pid = match child.id() {
            Some(pid) => pid as i32,
            None => {
                release_taps(&ports).await;
                let _ = std::fs::remove_file(&socket);
                if prior.is_none() {
                    let _ = tokio::fs::remove_dir_all(&dir).await;
                }
                return Err(ApiError::unavailable(
                    "cloud-hypervisor runtime: process did not start",
                ));
            }
        };
        let stderr = child.stderr.take();
        match tokio::time::timeout(PROBE, child.wait()).await {
            Ok(Ok(status)) => {
                let tail = stderr_tail(&read_stderr(stderr).await);
                release_taps(&ports).await;
                let _ = std::fs::remove_file(&socket);
                if prior.is_none() {
                    let _ = tokio::fs::remove_dir_all(&dir).await;
                }
                let why = if tail.is_empty() {
                    format!("exit {status}")
                } else {
                    tail
                };
                return Err(ApiError::unavailable(format!(
                    "cloud-hypervisor runtime: process exited: {why}"
                )));
            }
            Ok(Err(e)) => {
                release_taps(&ports).await;
                let _ = std::fs::remove_file(&socket);
                if prior.is_none() {
                    let _ = tokio::fs::remove_dir_all(&dir).await;
                }
                return Err(ApiError::unavailable(format!(
                    "cloud-hypervisor runtime: {e}"
                )));
            }
            Err(_) => {
                if !pid_alive(pid) {
                    release_taps(&ports).await;
                    let _ = std::fs::remove_file(&socket);
                    if prior.is_none() {
                        let _ = tokio::fs::remove_dir_all(&dir).await;
                    }
                    return Err(ApiError::unavailable(
                        "cloud-hypervisor runtime: process exited",
                    ));
                }
                if let Some(stderr) = stderr {
                    tokio::spawn(async move {
                        let mut stderr = stderr;
                        let mut buf = [0u8; 1024];
                        loop {
                            match stderr.read(&mut buf).await {
                                Ok(0) | Err(_) => break,
                                Ok(_) => {}
                            }
                        }
                    });
                }
            }
        }

        let addresses = nic_addresses(req);
        write_state(
            &dir,
            &Saved {
                id: id.to_string(),
                name: req.name.clone(),
                addresses: addresses.clone(),
                power: PowerState::On,
                pid,
                socket: socket.to_string_lossy().into_owned(),
                nets: ports.iter().map(SavedNet::from).collect(),
            },
        )
        .await?;
        Ok(RunningMachine {
            id,
            addresses,
            power: PowerState::On,
        })
    }

    async fn stop(&self, id: &Id) -> ApiResult<()> {
        let _guard = lock().lock().await;
        let _bin = resolve_bin()?;
        let dir = machine_dir(id)?;
        let Some(mut saved) = read_state(&dir).await? else {
            return Err(ApiError::not_found(format!(
                "cloud-hypervisor runtime: machine {id} is not on this host"
            )));
        };
        if protected_name(&saved.name) {
            return Err(host_guard());
        }
        if saved.power == PowerState::Off && !pid_alive(saved.pid) && saved.nets.is_empty() {
            return Ok(());
        }
        if pid_alive(saved.pid) {
            stop_pid(saved.pid).await;
        }
        let (left, err) = delete_taps(&saved.nets).await;
        saved.power = PowerState::Off;
        saved.pid = 0;
        saved.nets = left;
        write_state(&dir, &saved).await?;
        match err {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    async fn delete(&self, id: &Id) -> ApiResult<()> {
        let _guard = lock().lock().await;
        let _bin = resolve_bin()?;
        let dir = machine_dir(id)?;
        if !dir.exists() {
            return Ok(());
        }
        if let Some(saved) = read_state(&dir).await? {
            if protected_name(&saved.name) {
                return Err(host_guard());
            }
            if pid_alive(saved.pid) {
                stop_pid(saved.pid).await;
            }
            let (left, err) = delete_taps(&saved.nets).await;
            if let Some(err) = err {
                let mut failed = saved.clone();
                failed.nets = left;
                failed.power = PowerState::Off;
                failed.pid = 0;
                write_state(&dir, &failed).await?;
                return Err(err);
            }
            if !saved.socket.is_empty() {
                let _ = std::fs::remove_file(&saved.socket);
            }
        }
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ApiError::unavailable(format!(
                "cloud-hypervisor runtime: removing {}: {e}",
                dir.display()
            ))),
        }
    }

    async fn list(&self) -> ApiResult<Vec<RunningMachine>> {
        let _guard = lock().lock().await;
        let _bin = resolve_bin()?;
        let root = state_dir();
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut rd = tokio::fs::read_dir(&root).await.map_err(|e| {
            ApiError::unavailable(format!(
                "cloud-hypervisor runtime: reading {}: {e}",
                root.display()
            ))
        })?;
        let mut out = Vec::new();
        loop {
            let ent = rd.next_entry().await.map_err(|e| {
                ApiError::unavailable(format!(
                    "cloud-hypervisor runtime: reading {}: {e}",
                    root.display()
                ))
            })?;
            let Some(ent) = ent else { break };
            let ft = ent.file_type().await.map_err(|e| {
                ApiError::unavailable(format!(
                    "cloud-hypervisor runtime: reading {}: {e}",
                    root.display()
                ))
            })?;
            if !ft.is_dir() {
                continue;
            }
            let Some(saved) = read_state(&ent.path()).await? else {
                continue;
            };
            let power = if saved.power == PowerState::On && !pid_alive(saved.pid) {
                PowerState::Off
            } else {
                saved.power
            };
            out.push(RunningMachine {
                id: parse_id(&saved.id)?,
                addresses: saved.addresses,
                power,
            });
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }
}

fn lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn state_dir() -> PathBuf {
    match std::env::var_os("PISTACK_CLOUD_STATE") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(DEFAULT_STATE),
    }
}

fn machine_dir(id: &Id) -> ApiResult<PathBuf> {
    let name = id.to_string();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(ApiError::bad_request(format!(
            "cloud-hypervisor runtime: machine id {name} cannot name a directory"
        )));
    }
    Ok(state_dir().join(name))
}

fn resolve_bin() -> ApiResult<PathBuf> {
    if let Some(v) = std::env::var_os("PISTACK_CLOUD_HYPERVISOR_BIN") {
        if !v.is_empty() {
            let path = PathBuf::from(&v);
            if is_executable(&path) {
                return Ok(path);
            }
            return Err(ApiError::unavailable(format!(
                "cloud-hypervisor runtime: PISTACK_CLOUD_HYPERVISOR_BIN {} is not available",
                path.display()
            )));
        }
    }
    crate::facts::which("cloud-hypervisor").ok_or_else(|| {
        ApiError::unavailable("cloud-hypervisor runtime: cloud-hypervisor is not on PATH")
    })
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

fn kernel_file() -> Option<PathBuf> {
    let v = std::env::var_os("PISTACK_CLOUD_KERNEL")?;
    if v.is_empty() {
        return None;
    }
    let path = PathBuf::from(v);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() == 0 {
        return None;
    }
    Some(path)
}

fn protected_name(name: &str) -> bool {
    matches!(
        std::env::var("PISTACK_NODE_NAME"),
        Ok(node) if !node.is_empty() && node == name
    )
}

fn host_guard() -> ApiError {
    ApiError::unavailable("this host is not powered off by the agent")
}

fn nic_addresses(req: &MachineRequest) -> Vec<String> {
    req.nics
        .iter()
        .map(|n| n.address.clone())
        .filter(|a| !a.is_empty())
        .collect()
}

/// Hot-plug one port onto a running guest. A missing machine, a stopped
/// guest, or a missing `ch-remote` is unavailable. The same port on the same
/// bridge is a success and does not call `ch-remote` again.
pub async fn plug_port(req: &PortPlug) -> ApiResult<()> {
    let _guard = lock().lock().await;
    let _bin = resolve_bin()?;
    let dir = machine_dir(&req.machine)?;
    let Some(mut saved) = read_state(&dir).await? else {
        return Err(ApiError::unavailable(format!(
            "cloud-hypervisor runtime: machine {} is not on this host",
            req.machine
        )));
    };
    if protected_name(&saved.name) {
        return Err(host_guard());
    }
    require_running(&saved)?;
    let socket = require_socket(&saved)?;
    let planned = plan_ports(std::slice::from_ref(&req.nic))?;
    let port = &planned[0];
    if let Some(existing) = saved
        .nets
        .iter()
        .find(|n| n.mac.eq_ignore_ascii_case(&port.mac))
    {
        if existing.bridge == port.bridge && existing.tap == port.tap {
            return Ok(());
        }
    }
    // A missing helper must leave the guest and its taps as they were.
    let remote = resolve_ch_remote()?;
    if let Some(existing) = saved
        .nets
        .iter()
        .find(|n| n.mac.eq_ignore_ascii_case(&port.mac))
        .cloned()
    {
        remove_net_device(&remote, &socket, &existing).await?;
        let _ = delete_tap(&existing.tap).await;
        saved
            .nets
            .retain(|n| !n.mac.eq_ignore_ascii_case(&port.mac));
        write_state(&dir, &saved).await?;
    }
    create_tap(&port.tap, &port.bridge).await?;
    let spec = net_arg(&port.tap, &port.mac, &port.id);
    match add_net(&remote, &socket, &spec).await {
        Ok(stdout) => {
            let mut net = SavedNet::from(port);
            net.id = parse_device_id(&stdout, &port.id);
            saved.nets.push(net);
            write_state(&dir, &saved).await
        }
        Err(e) => {
            let _ = delete_tap(&port.tap).await;
            Err(e)
        }
    }
}

/// Remove the port with this MAC. A missing machine is unavailable. A port
/// that is already gone is success. A running guest uses `ch-remote
/// remove-device` when that binary is available.
pub async fn unplug_port(req: &PortPlug) -> ApiResult<()> {
    let _guard = lock().lock().await;
    let _bin = resolve_bin()?;
    let dir = machine_dir(&req.machine)?;
    let Some(mut saved) = read_state(&dir).await? else {
        return Err(ApiError::unavailable(format!(
            "cloud-hypervisor runtime: machine {} is not on this host",
            req.machine
        )));
    };
    if protected_name(&saved.name) {
        return Err(host_guard());
    }
    if mac_hex(&req.nic.mac).is_none() {
        return Err(ApiError::bad_request("nic mac is not an address"));
    }
    let Some(idx) = saved
        .nets
        .iter()
        .position(|n| n.mac.eq_ignore_ascii_case(&req.nic.mac))
    else {
        return Ok(());
    };
    let net = saved.nets[idx].clone();
    if saved.power == PowerState::On && pid_alive(saved.pid) {
        let socket = require_socket(&saved)?;
        let remote = resolve_ch_remote()?;
        remove_net_device(&remote, &socket, &net).await?;
    }
    delete_tap(&net.tap).await?;
    saved.nets.remove(idx);
    write_state(&dir, &saved).await
}

fn require_running(saved: &Saved) -> ApiResult<()> {
    if saved.power == PowerState::On && pid_alive(saved.pid) {
        Ok(())
    } else {
        Err(ApiError::unavailable(format!(
            "cloud-hypervisor runtime: machine {} is not running",
            saved.id
        )))
    }
}

fn require_socket(saved: &Saved) -> ApiResult<PathBuf> {
    let path = PathBuf::from(&saved.socket);
    if saved.socket.is_empty() || !path.exists() {
        return Err(ApiError::unavailable(
            "cloud-hypervisor runtime: api socket is not available",
        ));
    }
    Ok(path)
}

fn plan_ports(nics: &[NicAttachment]) -> ApiResult<Vec<PortTap>> {
    let mut out: Vec<PortTap> = Vec::with_capacity(nics.len());
    for nic in nics {
        let bridge = crate::runtime::libvirt::port_bridge(nic)?;
        if !bridge_name_ok(&bridge) {
            return Err(ApiError::bad_request(
                "cloud-hypervisor runtime: port bridge is not allowed",
            ));
        }
        let tap = tap_name(&nic.mac)?;
        if out.iter().any(|port| port.tap == tap) {
            return Err(ApiError::bad_request(
                "cloud-hypervisor runtime: nic mac is already attached",
            ));
        }
        out.push(PortTap {
            mac: nic.mac.clone(),
            tap,
            bridge,
            id: device_id_for(&nic.mac)?,
        });
    }
    Ok(out)
}

fn net_arg(tap: &str, mac: &str, id: &str) -> String {
    format!("tap={tap},mac={mac},id={id}")
}

/// `ct` plus the 12 hex digits of the MAC. The name fits in an interface
/// name and does not start with `pi`.
fn tap_name(mac: &str) -> ApiResult<String> {
    let hex = mac_hex(mac).ok_or_else(|| ApiError::bad_request("nic mac is not an address"))?;
    let name = format!("ct{hex}");
    if !tap_name_ok(&name) {
        return Err(ApiError::bad_request("nic mac is not an address"));
    }
    Ok(name)
}

fn device_id_for(mac: &str) -> ApiResult<String> {
    let hex = mac_hex(mac).ok_or_else(|| ApiError::bad_request("nic mac is not an address"))?;
    Ok(format!("n{hex}"))
}

fn mac_hex(mac: &str) -> Option<String> {
    let parts: Vec<&str> = mac.split(':').collect();
    if parts.len() != 6
        || !parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return None;
    }
    Some(
        parts
            .iter()
            .flat_map(|p| p.chars())
            .map(|c| c.to_ascii_lowercase())
            .collect(),
    )
}

fn tap_name_ok(name: &str) -> bool {
    let Some(hex) = name.strip_prefix("ct") else {
        return false;
    };
    name.len() == 14 && hex.len() == 12 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// `pistack0`, or `pi` / `po` plus 12 hex digits. Nothing else is passed to `ip`.
fn bridge_name_ok(name: &str) -> bool {
    if name == "pistack0" {
        return true;
    }
    let hex = if let Some(rest) = name.strip_prefix("po") {
        rest
    } else if let Some(rest) = name.strip_prefix("pi") {
        rest
    } else {
        return false;
    };
    hex.len() == 12 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn device_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn parse_device_id(stdout: &str, fallback: &str) -> String {
    let trimmed = stdout.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(id) = v.get("id").and_then(|s| s.as_str()) {
            if device_id_ok(id) {
                return id.to_owned();
            }
        }
    }
    if device_id_ok(trimmed) {
        return trimmed.to_owned();
    }
    fallback.to_owned()
}

fn ip_bin() -> ApiResult<PathBuf> {
    if let Some(v) = std::env::var_os("PISTACK_IP_BIN") {
        if !v.is_empty() {
            let path = PathBuf::from(&v);
            if is_executable(&path) {
                return Ok(path);
            }
            return Err(ApiError::unavailable(
                "cloud-hypervisor runtime: ip helper is not available",
            ));
        }
    }
    crate::facts::which("ip")
        .ok_or_else(|| ApiError::unavailable("cloud-hypervisor runtime: ip is not available"))
}

fn resolve_ch_remote() -> ApiResult<PathBuf> {
    if let Some(v) = std::env::var_os("PISTACK_CH_REMOTE") {
        if !v.is_empty() {
            let path = PathBuf::from(&v);
            if is_executable(&path) {
                return Ok(path);
            }
            return Err(ApiError::unavailable(
                "cloud-hypervisor runtime: ch-remote is not available",
            ));
        }
    }
    crate::facts::which("ch-remote").ok_or_else(|| {
        ApiError::unavailable("cloud-hypervisor runtime: ch-remote is not available")
    })
}

async fn create_ports(ports: &[PortTap]) -> ApiResult<()> {
    for (i, port) in ports.iter().enumerate() {
        if let Err(e) = create_tap(&port.tap, &port.bridge).await {
            release_taps(&ports[..i]).await;
            return Err(e);
        }
    }
    Ok(())
}

async fn create_tap(tap: &str, bridge: &str) -> ApiResult<()> {
    if !tap_name_ok(tap) || !bridge_name_ok(bridge) {
        return Err(ApiError::unavailable(
            "cloud-hypervisor runtime: tap bridge is not allowed",
        ));
    }
    let bin = ip_bin()?;
    // A crash can leave this port's tap behind. It is ours to replace.
    match run_tool(&bin, &["link", "del", tap]).await {
        Ok(_) => {}
        Err(e) if tap_missing(&e.message) => {}
        Err(e) => return Err(e),
    }
    run_tool(&bin, &["tuntap", "add", "mode", "tap", "name", tap]).await?;
    if let Err(e) = run_tool(&bin, &["link", "set", tap, "master", bridge]).await {
        let _ = delete_tap(tap).await;
        return Err(e);
    }
    if let Err(e) = run_tool(&bin, &["link", "set", tap, "up"]).await {
        let _ = delete_tap(tap).await;
        return Err(e);
    }
    Ok(())
}

async fn release_taps(ports: &[PortTap]) {
    for port in ports {
        let _ = delete_tap(&port.tap).await;
    }
}

async fn delete_taps(nets: &[SavedNet]) -> (Vec<SavedNet>, Option<ApiError>) {
    let mut left = Vec::new();
    let mut err = None;
    for net in nets {
        match delete_tap(&net.tap).await {
            Ok(()) => {}
            Err(e) => {
                if err.is_none() {
                    err = Some(e);
                }
                left.push(net.clone());
            }
        }
    }
    (left, err)
}

async fn delete_tap(name: &str) -> ApiResult<()> {
    if !tap_name_ok(name) {
        return Ok(());
    }
    let bin = ip_bin()?;
    match run_tool(&bin, &["link", "del", name]).await {
        Ok(_) => Ok(()),
        Err(e) if tap_missing(&e.message) => Ok(()),
        Err(e) => Err(e),
    }
}

fn tap_missing(msg: &str) -> bool {
    let msg = msg.to_ascii_lowercase();
    msg.contains("cannot find device")
        || msg.contains("does not exist")
        || msg.contains("no such device")
}

async fn add_net(remote: &Path, socket: &Path, spec: &str) -> ApiResult<String> {
    let sock = socket.to_string_lossy().into_owned();
    run_tool(remote, &["--api-socket", &sock, "add-net", spec]).await
}

async fn remove_net_device(remote: &Path, socket: &Path, net: &SavedNet) -> ApiResult<()> {
    if !device_id_ok(&net.id) {
        return Err(ApiError::unavailable(
            "cloud-hypervisor runtime: net device id is not available",
        ));
    }
    let sock = socket.to_string_lossy().into_owned();
    run_tool(remote, &["--api-socket", &sock, "remove-device", &net.id])
        .await
        .map(|_| ())
}

async fn run_tool(bin: &Path, args: &[&str]) -> ApiResult<String> {
    let child = Command::new(bin)
        .args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let name = bin.file_name().and_then(|s| s.to_str()).unwrap_or("helper");
    match tokio::time::timeout(TOOL_WAIT, child).await {
        Ok(Ok(out)) if out.status.success() => {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        Ok(Ok(out)) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let msg = trim_msg(err.trim());
            Err(ApiError::unavailable(if msg.is_empty() {
                format!("cloud-hypervisor runtime: {name} failed")
            } else {
                format!("cloud-hypervisor runtime: {name} failed: {msg}")
            }))
        }
        Ok(Err(e)) => Err(ApiError::unavailable(format!(
            "cloud-hypervisor runtime: cannot run {name}: {e}"
        ))),
        Err(_) => Err(ApiError::unavailable(format!(
            "cloud-hypervisor runtime: {name} timed out"
        ))),
    }
}

fn trim_msg(msg: &str) -> String {
    msg.chars().take(500).collect()
}

impl From<&PortTap> for SavedNet {
    fn from(port: &PortTap) -> Self {
        Self {
            mac: port.mac.clone(),
            tap: port.tap.clone(),
            bridge: port.bridge.clone(),
            id: port.id.clone(),
        }
    }
}

fn parse_id(s: &str) -> ApiResult<Id> {
    Id::parse(kind::MACHINE, s).ok_or_else(|| {
        ApiError::unavailable(format!(
            "cloud-hypervisor runtime: state id {s} is not a machine id"
        ))
    })
}

/// Disk image: an existing, non-empty, absolute regular file. Symlinks and
/// any `..` component are refused and are not followed.
fn validate_disk(image: &str) -> ApiResult<PathBuf> {
    let path = Path::new(image);
    if image.is_empty() || !path.is_absolute() || has_parent_dir(path) {
        return Err(ApiError::unavailable(
            "cloud-hypervisor runtime: image is not available",
        ));
    }
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => {
            return Err(ApiError::unavailable(
                "cloud-hypervisor runtime: image is not available",
            ))
        }
    };
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() == 0 {
        return Err(ApiError::unavailable(
            "cloud-hypervisor runtime: image is not available",
        ));
    }
    Ok(path.to_path_buf())
}

fn has_parent_dir(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
        || path.to_string_lossy().split('/').any(|p| p == "..")
}

/// `ch` plus the first 12 hex digits of the id, under the temp directory.
/// The name does not start with `pi`.
fn socket_path(id: &Id) -> ApiResult<PathBuf> {
    let file = format!("ch{}.sock", short_hex(&id.to_string()));
    let mut path = std::env::temp_dir().join(&file);
    if path.as_os_str().len() > 100 {
        path = PathBuf::from("/tmp").join(&file);
    }
    if path.as_os_str().len() > 100 {
        return Err(ApiError::unavailable(
            "cloud-hypervisor runtime: api socket path is too long",
        ));
    }
    Ok(path)
}

fn short_hex(id: &str) -> String {
    let rest = match id.split_once('-') {
        Some((left, right))
            if !left.is_empty()
                && !right.is_empty()
                && left.chars().all(|c| c.is_ascii_alphabetic()) =>
        {
            right
        }
        _ => id,
    };
    rest.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .flat_map(|c| c.to_lowercase())
        .take(12)
        .collect()
}

fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

async fn stop_pid(pid: i32) {
    if !pid_alive(pid) {
        reap(pid);
        return;
    }
    unsafe { libc::kill(pid, libc::SIGTERM) };
    for i in 0..10 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        reap(pid);
        if !pid_alive(pid) {
            return;
        }
        if i == 4 {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
    reap(pid);
}

fn reap(pid: i32) {
    if pid <= 0 {
        return;
    }
    unsafe {
        libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG);
    }
}

async fn read_state(dir: &Path) -> ApiResult<Option<Saved>> {
    let path = dir.join("state.json");
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let saved = serde_json::from_slice(&bytes).map_err(|e| {
                ApiError::unavailable(format!(
                    "cloud-hypervisor runtime: state {}: {e}",
                    path.display()
                ))
            })?;
            Ok(Some(saved))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ApiError::unavailable(format!(
            "cloud-hypervisor runtime: reading {}: {e}",
            path.display()
        ))),
    }
}

async fn write_state(dir: &Path, saved: &Saved) -> ApiResult<()> {
    let path = dir.join("state.json");
    let text = serde_json::to_vec_pretty(saved).map_err(|e| {
        ApiError::internal(format!("cloud-hypervisor runtime: encoding state: {e}"))
    })?;
    tokio::fs::write(&path, text).await.map_err(|e| {
        ApiError::unavailable(format!(
            "cloud-hypervisor runtime: writing {}: {e}",
            path.display()
        ))
    })
}

async fn read_stderr(stderr: Option<tokio::process::ChildStderr>) -> Vec<u8> {
    let Some(mut stderr) = stderr else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_millis(200), async {
        let mut tmp = [0u8; 1024];
        loop {
            if buf.len() > 4000 {
                break;
            }
            match stderr.read(&mut tmp).await {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
        }
    })
    .await;
    buf
}

fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(20);
    let mut tail = lines[start..].join("\n");
    const MAX: usize = 1500;
    if tail.chars().count() > MAX {
        let skip = tail.chars().count() - MAX;
        tail = tail.chars().skip(skip).collect();
    }
    tail
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{select, select_runtimes, MachineKind, NicAttachment};
    use pistack_rpc::proto::PortPlug;
    use pistack_types::{error::ErrorCode, Resources};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::OnceLock;

    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    struct EnvGuard {
        bin: Option<std::ffi::OsString>,
        state: Option<std::ffi::OsString>,
        kernel: Option<std::ffi::OsString>,
        node: Option<std::ffi::OsString>,
        ip: Option<std::ffi::OsString>,
        remote: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(bin: &Path, state: &Path) -> Self {
            let prev = Self {
                bin: std::env::var_os("PISTACK_CLOUD_HYPERVISOR_BIN"),
                state: std::env::var_os("PISTACK_CLOUD_STATE"),
                kernel: std::env::var_os("PISTACK_CLOUD_KERNEL"),
                node: std::env::var_os("PISTACK_NODE_NAME"),
                ip: std::env::var_os("PISTACK_IP_BIN"),
                remote: std::env::var_os("PISTACK_CH_REMOTE"),
            };
            std::env::set_var("PISTACK_CLOUD_HYPERVISOR_BIN", bin);
            std::env::set_var("PISTACK_CLOUD_STATE", state);
            std::env::remove_var("PISTACK_CLOUD_KERNEL");
            std::env::remove_var("PISTACK_IP_BIN");
            std::env::remove_var("PISTACK_CH_REMOTE");
            prev
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            restore("PISTACK_CLOUD_HYPERVISOR_BIN", self.bin.take());
            restore("PISTACK_CLOUD_STATE", self.state.take());
            restore("PISTACK_CLOUD_KERNEL", self.kernel.take());
            restore("PISTACK_NODE_NAME", self.node.take());
            restore("PISTACK_IP_BIN", self.ip.take());
            restore("PISTACK_CH_REMOTE", self.remote.take());
        }
    }

    fn restore(key: &str, prev: Option<std::ffi::OsString>) {
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    struct DirGuard {
        dir: PathBuf,
        state: PathBuf,
    }

    impl Drop for DirGuard {
        fn drop(&mut self) {
            if let Ok(rd) = std::fs::read_dir(&self.state) {
                for ent in rd.flatten() {
                    let path = ent.path().join("state.json");
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(pid) = v.get("pid").and_then(|p| p.as_i64()) {
                                let pid = pid as i32;
                                unsafe {
                                    libc::kill(pid, libc::SIGKILL);
                                    libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG);
                                }
                            }
                            if let Some(sock) = v.get("socket").and_then(|s| s.as_str()) {
                                let _ = std::fs::remove_file(sock);
                            }
                        }
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn write_script(path: &Path, log: &Path, body: &str) {
        let quoted = format!("'{}'", log.display().to_string().replace('\'', "'\\''"));
        let text = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> {quoted}\n{body}\n");
        std::fs::write(path, text).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    fn req(name: &str, image: &str) -> MachineRequest {
        MachineRequest {
            id: Id::new(kind::MACHINE),
            name: name.into(),
            kind: MachineKind::Vm,
            image: image.into(),
            resources: Resources {
                vcpus: 1,
                memory_mib: 128,
                disk_gib: 1,
                gpus: 0,
            },
            networks: vec![],
            nics: vec![NicAttachment {
                network_id: "net-01a102de-3f33-73bf-86f7-57fe3b47f1ea".into(),
                mac: "52:54:00:aa:bb:cc".into(),
                address: "10.9.0.4".into(),
                prefix: 24,
                gateway: "10.9.0.1".into(),
                routed: true,
                segment_id: None,
            }],
            volumes: vec![],
            user_data: Some("do-not-log-user-data".into()),
            ssh_keys: vec!["do-not-log-ssh-key".into()],
            overlay: None,
            bmc_address: None,
            bmc_user: None,
        }
    }

    fn lines_of(log: &str) -> Vec<String> {
        log.lines()
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn has_seq(log: &str, seq: &[&str]) -> bool {
        count_seq(log, seq) > 0
    }

    fn count_seq(log: &str, seq: &[&str]) -> usize {
        let lines: Vec<&str> = log.lines().filter(|l| !l.is_empty()).collect();
        lines.windows(seq.len()).filter(|w| *w == seq).count()
    }

    fn nic(address: &str, mac: &str, segment: Option<u32>) -> NicAttachment {
        NicAttachment {
            network_id: "net-01a102de-3f33-73bf-86f7-57fe3b47f1ea".into(),
            mac: mac.into(),
            address: address.into(),
            prefix: 24,
            gateway: "10.90.0.1".into(),
            routed: true,
            segment_id: segment,
        }
    }

    #[test]
    fn cloud_hypervisor_is_selected_only_without_libvirt() {
        let only = ["cloud-hypervisor".to_owned()];
        assert_eq!(
            select(MachineKind::Vm, &only, false).unwrap().name(),
            "cloud-hypervisor"
        );
        assert_eq!(
            select_runtimes(&only, false)
                .get(MachineKind::Vm)
                .unwrap()
                .name(),
            "cloud-hypervisor"
        );
        assert!(select(MachineKind::Container, &only, false).is_none());
        for cap in ["kvm", "libvirt"] {
            let both = [cap.to_owned(), "cloud-hypervisor".to_owned()];
            assert_eq!(
                select(MachineKind::Vm, &both, false).unwrap().name(),
                "libvirt",
                "{cap}"
            );
            assert_eq!(
                select_runtimes(&both, false)
                    .get(MachineKind::Vm)
                    .unwrap()
                    .name(),
                "libvirt"
            );
        }
        let youki = ["youki".to_owned(), "cloud-hypervisor".to_owned()];
        assert_eq!(
            select(MachineKind::Container, &youki, false)
                .unwrap()
                .name(),
            "youki"
        );
    }

    #[tokio::test]
    async fn start_runs_until_stop_and_missing_pieces_are_unavailable() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-cloud-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let state = tmp.join("state");
        let _dir = DirGuard {
            dir: tmp.clone(),
            state: state.clone(),
        };
        let log = tmp.join("ch.log");
        let bin = tmp.join("cloud-hypervisor");
        write_script(&bin, &log, "exec sleep 30");
        let ip_log = tmp.join("ip.log");
        let ip = tmp.join("ip");
        write_script(&ip, &ip_log, "exit 0");
        let _env = EnvGuard::set(&bin, &state);
        std::env::set_var("PISTACK_IP_BIN", &ip);

        let rt = CloudRuntime;
        let unknown = Id::new(kind::MACHINE);
        assert_eq!(
            rt.stop(&unknown).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        rt.delete(&unknown).await.unwrap();

        let disk = tmp.join("disk.raw");
        std::fs::write(&disk, b"not-empty").unwrap();
        let kernel = tmp.join("vmlinux");
        std::fs::write(&kernel, b"kernel").unwrap();
        std::env::set_var("PISTACK_CLOUD_KERNEL", &kernel);

        let request = req("web", disk.to_str().unwrap());
        let started = rt.start(&request).await.unwrap();
        assert_eq!(started.power, PowerState::On);
        assert_eq!(started.addresses, vec!["10.9.0.4".to_string()]);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(!text.contains("do-not-log-user-data"));
        assert!(!text.contains("do-not-log-ssh-key"));
        assert!(lines_of(&text).windows(2).any(|w| {
            w[0] == "--net" && w[1] == "tap=ct525400aabbcc,mac=52:54:00:aa:bb:cc,id=n525400aabbcc"
        }));
        let ip_text = std::fs::read_to_string(&ip_log).unwrap();
        assert!(
            has_seq(
                &ip_text,
                &["link", "set", "ct525400aabbcc", "master", "pi01a102de3f33",],
            ),
            "{ip_text}"
        );
        assert!(!ip_text.contains("do-not-log-user-data"));
        assert!(!ip_text.contains("do-not-log-ssh-key"));
        assert!(!ip_text.contains("pistack-fip"));
        let lines = lines_of(&text);
        let sock = lines
            .windows(2)
            .find(|w| w[0] == "--api-socket")
            .map(|w| w[1].clone())
            .unwrap();
        let sock_path = Path::new(&sock);
        let sock_name = sock_path.file_name().unwrap().to_string_lossy();
        assert!(sock_name.starts_with("ch"), "{sock_name}");
        assert!(sock_name.ends_with(".sock"), "{sock_name}");
        assert!(!sock_name.starts_with("pi"), "{sock_name}");
        assert!(sock_path.starts_with(std::env::temp_dir()) || sock_path.starts_with("/tmp"));
        assert!(lines
            .windows(2)
            .any(|w| w[0] == "--disk" && w[1] == format!("path={}", disk.display())));
        assert!(lines
            .windows(2)
            .any(|w| w[0] == "--cpus" && w[1] == "boot=1"));
        assert!(lines
            .windows(2)
            .any(|w| w[0] == "--memory" && w[1] == "size=128M"));
        assert!(lines
            .windows(2)
            .any(|w| w[0] == "--console" && w[1] == "off"));
        assert!(lines
            .windows(2)
            .any(|w| w[0] == "--serial" && w[1] == "off"));
        assert!(lines
            .windows(2)
            .any(|w| w[0] == "--kernel" && w[1] == kernel.display().to_string()));
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(state.join(request.id.to_string()).join("state.json")).unwrap(),
        )
        .unwrap();
        assert!(!saved.to_string().contains("do-not-log-user-data"));
        assert!(!saved.to_string().contains("do-not-log-ssh-key"));

        let again = rt.start(&request).await.unwrap();
        assert_eq!(again.power, PowerState::On);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), text);
        assert_eq!(std::fs::read_to_string(&ip_log).unwrap(), ip_text);

        assert_eq!(rt.list().await.unwrap()[0].power, PowerState::On);
        let dels = count_seq(&ip_text, &["link", "del", "ct525400aabbcc"]);
        rt.stop(&request.id).await.unwrap();
        assert_eq!(rt.list().await.unwrap()[0].power, PowerState::Off);
        let stopped = std::fs::read_to_string(&ip_log).unwrap();
        assert_eq!(
            count_seq(&stopped, &["link", "del", "ct525400aabbcc"]),
            dels + 1,
            "{stopped}"
        );
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(state.join(request.id.to_string()).join("state.json")).unwrap(),
        )
        .unwrap();
        assert!(saved["nets"].as_array().unwrap().is_empty());

        let before = std::fs::read_to_string(&log).unwrap();
        std::env::set_var("PISTACK_NODE_NAME", "web");
        let refused = rt.start(&request).await.unwrap_err();
        assert_eq!(refused.message, "this host is not powered off by the agent");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), before);
        std::env::remove_var("PISTACK_NODE_NAME");

        let missing_img = req("img", &tmp.join("no-such-disk").display().to_string());
        let err = rt.start(&missing_img).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(!err.message.contains("do-not-log"));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), before);
        assert!(rt
            .list()
            .await
            .unwrap()
            .iter()
            .all(|m| m.id != missing_img.id));

        std::os::unix::fs::symlink(&disk, tmp.join("link-disk")).unwrap();
        let link_req = req("link", tmp.join("link-disk").to_str().unwrap());
        assert_eq!(
            rt.start(&link_req).await.unwrap_err().code,
            ErrorCode::Unavailable
        );
        let dotted = format!(
            "{}/../{}/disk.raw",
            tmp.parent().unwrap().display(),
            tmp.file_name().unwrap().to_string_lossy()
        );
        assert_eq!(
            rt.start(&req("dot", &dotted)).await.unwrap_err().code,
            ErrorCode::Unavailable
        );
        std::fs::write(tmp.join("empty-disk"), b"").unwrap();
        assert_eq!(
            rt.start(&req("empty", tmp.join("empty-disk").to_str().unwrap()))
                .await
                .unwrap_err()
                .code,
            ErrorCode::Unavailable
        );

        let missing = tmp.join("missing-cloud");
        std::env::set_var("PISTACK_CLOUD_HYPERVISOR_BIN", &missing);
        for err in [
            rt.start(&request).await.unwrap_err(),
            rt.stop(&request.id).await.unwrap_err(),
            rt.delete(&request.id).await.unwrap_err(),
            rt.list().await.unwrap_err(),
        ] {
            assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        }
        std::env::set_var("PISTACK_CLOUD_HYPERVISOR_BIN", &bin);
        rt.delete(&request.id).await.unwrap();
        rt.delete(&request.id).await.unwrap();
        assert_eq!(
            rt.stop(&request.id).await.unwrap_err().code,
            ErrorCode::NotFound
        );

        let fail = tmp.join("cloud-fail");
        let fail_log = tmp.join("fail.log");
        write_script(&fail, &fail_log, "echo exploded >&2\nexit 1");
        std::env::set_var("PISTACK_CLOUD_HYPERVISOR_BIN", &fail);
        let bad = req("bad", disk.to_str().unwrap());
        let before_fail = count_seq(
            &std::fs::read_to_string(&ip_log).unwrap(),
            &["link", "del", "ct525400aabbcc"],
        );
        let err = rt.start(&bad).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("exploded"), "{}", err.message);
        assert!(!state.join(bad.id.to_string()).join("state.json").exists());
        assert!(rt
            .list()
            .await
            .unwrap()
            .iter()
            .all(|m| m.power != PowerState::On));
        let failed = std::fs::read_to_string(&ip_log).unwrap();
        // The tap is created for the guest, then removed when the process exits.
        assert_eq!(
            count_seq(&failed, &["link", "del", "ct525400aabbcc"]),
            before_fail + 2,
            "{failed}"
        );
    }

    fn touch_socket_body() -> &'static str {
        r#"sock=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--api-socket" ]; then
    sock=$a
  fi
  prev=$a
done
if [ -n "$sock" ]; then
  : > "$sock"
fi
exec sleep 30"#
    }

    #[tokio::test]
    async fn ports_are_taps_on_the_bridge_and_hotplug_uses_ch_remote() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-cloud-net-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let state = tmp.join("state");
        let _dir = DirGuard {
            dir: tmp.clone(),
            state: state.clone(),
        };
        let log = tmp.join("ch.log");
        let bin = tmp.join("cloud-hypervisor");
        write_script(&bin, &log, touch_socket_body());
        let ip_log = tmp.join("ip.log");
        let ip = tmp.join("ip");
        write_script(&ip, &ip_log, "exit 0");
        let remote_log = tmp.join("remote.log");
        let remote = tmp.join("ch-remote");
        write_script(
            &remote,
            &remote_log,
            "case \"$3\" in\n  add-net) printf '%s\\n' '{\"id\":\"nfromremote1\",\"bdf\":\"0000:00:04.0\"}' ;;\nesac\nexit 0",
        );
        let _env = EnvGuard::set(&bin, &state);
        std::env::set_var("PISTACK_IP_BIN", &ip);
        std::env::set_var("PISTACK_CH_REMOTE", &remote);

        let disk = tmp.join("disk.raw");
        std::fs::write(&disk, b"not-empty").unwrap();
        let rt = CloudRuntime;

        let mut request = req("ports", disk.to_str().unwrap());
        request.nics = vec![
            nic("10.88.0.10", "52:54:00:11:22:33", None),
            nic("10.88.0.11", "52:54:00:aa:bb:cc", Some(4)),
        ];
        rt.start(&request).await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(!text.contains("do-not-log-user-data"));
        assert!(!text.contains("do-not-log-ssh-key"));
        let lines = lines_of(&text);
        assert!(lines.windows(2).any(|w| {
            w[0] == "--net" && w[1] == "tap=ct525400112233,mac=52:54:00:11:22:33,id=n525400112233"
        }));
        assert!(lines.windows(2).any(|w| {
            w[0] == "--net" && w[1] == "tap=ct525400aabbcc,mac=52:54:00:aa:bb:cc,id=n525400aabbcc"
        }));
        let ip_text = std::fs::read_to_string(&ip_log).unwrap();
        assert!(has_seq(
            &ip_text,
            &["tuntap", "add", "mode", "tap", "name", "ct525400112233",],
        ));
        assert!(has_seq(
            &ip_text,
            &["link", "set", "ct525400112233", "master", "pistack0"],
        ));
        assert!(has_seq(&ip_text, &["link", "set", "ct525400112233", "up"],));
        assert!(has_seq(
            &ip_text,
            &["link", "set", "ct525400aabbcc", "master", "po01a102de3f33",],
        ));
        assert!(!has_seq(
            &ip_text,
            &["link", "set", "ct525400aabbcc", "master", "pistack0"],
        ));
        assert!(!ip_text.contains("pistack-fip"));
        assert!(!text.contains("pistack-fip"));

        let shared_dels = count_seq(&ip_text, &["link", "del", "ct525400112233"]);
        let overlay_dels = count_seq(&ip_text, &["link", "del", "ct525400aabbcc"]);
        rt.delete(&request.id).await.unwrap();
        let after_delete = std::fs::read_to_string(&ip_log).unwrap();
        assert!(
            count_seq(&after_delete, &["link", "del", "ct525400112233"]) > shared_dels,
            "{after_delete}"
        );
        assert!(
            count_seq(&after_delete, &["link", "del", "ct525400aabbcc"]) > overlay_dels,
            "{after_delete}"
        );
        assert!(!state.join(request.id.to_string()).exists());

        let mut bare = req("bare", disk.to_str().unwrap());
        bare.nics.clear();
        let _ = std::fs::remove_file(&log);
        rt.start(&bare).await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(!text.contains("--net"), "{text}");
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(state.join(bare.id.to_string()).join("state.json")).unwrap(),
        )
        .unwrap();
        let socket = saved["socket"].as_str().unwrap().to_owned();
        assert!(Path::new(&socket).exists(), "{socket}");

        let project = PortPlug {
            machine: bare.id.clone(),
            nic: nic("10.90.0.10", "52:54:00:aa:bb:cc", None),
        };
        plug_port(&project).await.unwrap();
        let ip_text = std::fs::read_to_string(&ip_log).unwrap();
        assert!(has_seq(
            &ip_text,
            &["link", "set", "ct525400aabbcc", "master", "pi01a102de3f33",],
        ));
        let remote_text = std::fs::read_to_string(&remote_log).unwrap();
        assert!(has_seq(&remote_text, &["--api-socket", &socket]));
        assert!(remote_text
            .contains("add-net\ntap=ct525400aabbcc,mac=52:54:00:aa:bb:cc,id=n525400aabbcc\n"));
        assert!(!remote_text.contains("do-not-log-user-data"));
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(state.join(bare.id.to_string()).join("state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["nets"][0]["id"], "nfromremote1");
        assert_eq!(saved["nets"][0]["bridge"], "pi01a102de3f33");

        let again = std::fs::read_to_string(&remote_log).unwrap();
        plug_port(&project).await.unwrap();
        assert_eq!(std::fs::read_to_string(&remote_log).unwrap(), again);

        let mut overlay = project.clone();
        overlay.nic.segment_id = Some(4);
        plug_port(&overlay).await.unwrap();
        let remote_text = std::fs::read_to_string(&remote_log).unwrap();
        assert!(
            remote_text.contains("remove-device\nnfromremote1\n"),
            "{remote_text}"
        );
        let ip_text = std::fs::read_to_string(&ip_log).unwrap();
        assert!(has_seq(
            &ip_text,
            &["link", "set", "ct525400aabbcc", "master", "po01a102de3f33",],
        ));
        assert!(!has_seq(
            &ip_text,
            &["link", "set", "ct525400aabbcc", "master", "pistack0"],
        ));
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(state.join(bare.id.to_string()).join("state.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["nets"][0]["bridge"], "po01a102de3f33");
        assert_eq!(saved["nets"][0]["id"], "nfromremote1");

        let parked = std::fs::read_to_string(&remote_log).unwrap();
        let parked_ip = std::fs::read_to_string(&ip_log).unwrap();
        plug_port(&overlay).await.unwrap();
        assert_eq!(std::fs::read_to_string(&remote_log).unwrap(), parked);
        assert_eq!(std::fs::read_to_string(&ip_log).unwrap(), parked_ip);

        let before_unplug = count_seq(&parked_ip, &["link", "del", "ct525400aabbcc"]);
        unplug_port(&overlay).await.unwrap();
        let remote_text = std::fs::read_to_string(&remote_log).unwrap();
        assert!(remote_text.contains("remove-device\nnfromremote1\n"));
        let ip_text = std::fs::read_to_string(&ip_log).unwrap();
        assert_eq!(
            count_seq(&ip_text, &["link", "del", "ct525400aabbcc"]),
            before_unplug + 1,
            "{ip_text}"
        );
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(state.join(bare.id.to_string()).join("state.json")).unwrap(),
        )
        .unwrap();
        assert!(saved["nets"].as_array().unwrap().is_empty());

        let parked = std::fs::read_to_string(&remote_log).unwrap();
        unplug_port(&overlay).await.unwrap();
        assert_eq!(std::fs::read_to_string(&remote_log).unwrap(), parked);

        let mut bad_mac = overlay.clone();
        bad_mac.nic.mac = "not-a-mac".into();
        let err = unplug_port(&bad_mac).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);

        let missing_remote = tmp.join("no-ch-remote");
        std::env::set_var("PISTACK_CH_REMOTE", &missing_remote);
        let ip_before = std::fs::read_to_string(&ip_log).unwrap();
        let err = plug_port(&overlay).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("ch-remote"), "{}", err.message);
        assert_eq!(std::fs::read_to_string(&ip_log).unwrap(), ip_before);
        std::env::set_var("PISTACK_CH_REMOTE", &remote);

        rt.stop(&bare.id).await.unwrap();
        let err = plug_port(&overlay).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("not running"), "{}", err.message);

        rt.delete(&bare.id).await.unwrap();
        let err = plug_port(&overlay).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("not on this host"), "{}", err.message);
        let err = unplug_port(&overlay).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("not on this host"), "{}", err.message);

        let mut bad = req("bad-mac", disk.to_str().unwrap());
        bad.nics = vec![nic("10.90.0.10", "not-a-mac", None)];
        let ch_before = std::fs::read_to_string(&log).unwrap();
        let ip_before = std::fs::read_to_string(&ip_log).unwrap();
        let err = rt.start(&bad).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), ch_before);
        assert_eq!(std::fs::read_to_string(&ip_log).unwrap(), ip_before);
    }
}
