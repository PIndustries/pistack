// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! youki (OCI) container runtime.
//!
//! One directory per machine under `$PISTACK_YOUKI_STATE` (default
//! `/var/lib/pistack/youki`). A pod is a sandbox bundle plus one bundle per
//! container, in spec order. The sandbox holds the pid, network, ipc and uts
//! namespaces; each container joins those and keeps its own mount namespace
//! so it has its own rootfs. `config.json` is the OCI spec, `rootfs` is the
//! unpacked image or the sandbox pause program, and `state.json` is written
//! after a successful start. The binary is `$PISTACK_YOUKI_BIN` when that is
//! set, otherwise `youki` on `PATH`. `user_data` and ssh keys are not logged
//! and are not placed on the youki argv. A missing youki binary, an image
//! that cannot be unpacked, or an empty rootfs is unavailable and is not
//! recorded as running. `exec` runs `youki exec` in one container of a pod
//! sandbox and returns that command's exit code, stdout, and stderr.
//! Command probes call that same helper, with the probe timeout and a small
//! output cap, and they do not pass an environment.
//!
//! Privilege. A root agent keeps this root OCI config and runs `youki`
//! directly. `PISTACK_YOUKI_SUDO` does not change that.
//!
//! When the agent is not root and `PISTACK_YOUKI_SUDO` is `1`, `true`, or
//! `yes`, every youki invocation (`create`, `start`, `kill`, `delete`,
//! `state`, `exec`) is `sudo -n <youki> ...` and the OCI config stays the root
//! config. Allow only that binary. `sudo -n` fails closed without the rule:
//!
//! ```text
//! # /etc/sudoers.d/pistack-youki
//! ubuntu ALL=(root) NOPASSWD: /usr/bin/youki
//! ```
//!
//! Replace `ubuntu` and `/usr/bin/youki` with the agent user and the youki
//! path on the host.
//!
//! When the agent is not root and sudo mode is off, the sandbox creates a
//! user namespace and maps container uid 0 and gid 0 onto the agent's uid and
//! gid with size 1. That map does not need `newuidmap`. Each pod container
//! joins the sandbox user namespace by path first, then the shared pid,
//! network, ipc and uts namespaces. Cgroup memory and cpu limits are written
//! only when that controller is delegated and the cgroup directory is
//! writable. `PISTACK_YOUKI_NETNS`, when it is an absolute path, is joined as
//! the network namespace; a rootless agent skips that path when it is owned
//! by another uid and keeps the sandbox's own network namespace.

use super::{MachineRequest, MachineRuntime, PowerState, RunningMachine};
use async_trait::async_trait;
use pistack_rpc::proto::ExecOutput;
use pistack_types::{kind, ApiError, ApiResult, Id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_STATE: &str = "/var/lib/pistack/youki";
/// Most bytes of stdout or stderr the agent keeps from one exec.
pub const EXEC_OUTPUT_CAP: usize = 256 * 1024;

/// Host-side OCI runtime. Stateless apart from the bundles on disk; a process
/// wide lock keeps start idempotent when two calls race.
#[derive(Debug, Default)]
pub struct YoukiRuntime;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Saved {
    id: String,
    #[serde(default)]
    addresses: Vec<String>,
    #[serde(default)]
    power: PowerState,
    /// youki ids of the pod's containers, in spec order. Empty for a
    /// machine that is not a pod sandbox.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    containers: Vec<String>,
    /// Container names in the same order as `containers`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    container_names: Vec<String>,
    /// Probe definitions only. The sandbox document itself is not stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    probes: Vec<crate::probe::ContainerProbes>,
}

#[async_trait]
impl MachineRuntime for YoukiRuntime {
    fn name(&self) -> &'static str {
        "youki"
    }

    async fn start(&self, req: &MachineRequest) -> ApiResult<RunningMachine> {
        let _guard = lock().lock().await;
        let bin = resolve_youki()?;
        let id = req.id.clone();
        let bundle = bundle_dir(&id)?;
        let prior = read_state(&bundle).await?;
        let probes = crate::probe::probes_in_user_data(req.user_data.as_deref());
        if let Some(saved) = prior.as_ref() {
            if saved.power == PowerState::On && parse_id(&saved.id)? == id {
                // Already running: do not exec youki again. Addresses come
                // from this request, not from whatever was stored. Refresh
                // the probe list so a restarted agent can keep checking.
                if saved.probes != probes {
                    let mut updated = saved.clone();
                    updated.probes = probes;
                    write_state(&bundle, &updated).await?;
                }
                return Ok(RunningMachine {
                    id,
                    addresses: nic_addresses(req),
                    power: PowerState::On,
                });
            }
        }
        let mode = resolve_run_mode()?;
        // user_data and ssh_keys are not written, logged, or passed to youki.
        if let Some(plan) = sandbox_plan(req)? {
            return start_pod(&bin, &mode, &bundle, req, &plan, prior.is_some()).await;
        }
        if let Err(e) = write_bundle(&bundle, req, &mode).await {
            if prior.is_none() {
                let _ = remove_bundle(&bundle).await;
            }
            return Err(e);
        }
        let id_str = id.to_string();
        let bundle_str = path_arg(&bundle);
        if prior.is_none() {
            exec_youki(
                &bin,
                &mode,
                &["create", "--bundle", &bundle_str, &id_str],
                None,
            )
            .await?;
            if let Err(e) = exec_youki(&bin, &mode, &["start", &id_str], None).await {
                // Create succeeded and start did not. Drop the container so a
                // retry can create it again, and do not record it as running.
                let _ = exec_youki(&bin, &mode, &["delete", &id_str], None).await;
                return Err(e);
            }
        } else {
            exec_youki(&bin, &mode, &["start", &id_str], None).await?;
        }
        let addresses = nic_addresses(req);
        write_state(
            &bundle,
            &Saved {
                id: id_str,
                addresses: addresses.clone(),
                power: PowerState::On,
                containers: Vec::new(),
                container_names: Vec::new(),
                probes: Vec::new(),
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
        let bin = resolve_youki()?;
        let bundle = bundle_dir(id)?;
        let Some(mut saved) = read_state(&bundle).await? else {
            return Err(ApiError::not_found(format!(
                "youki runtime: machine {id} is not on this host"
            )));
        };
        if saved.power == PowerState::Off {
            return Ok(());
        }
        let mode = resolve_run_mode()?;
        // Containers first, then the sandbox that owns their namespaces.
        let mut order: Vec<String> = saved.containers.iter().rev().cloned().collect();
        order.push(id.to_string());
        for cid in &order {
            exec_youki(&bin, &mode, &["kill", cid, "SIGTERM"], None).await?;
        }
        saved.power = PowerState::Off;
        write_state(&bundle, &saved).await
    }

    async fn delete(&self, id: &Id) -> ApiResult<()> {
        let _guard = lock().lock().await;
        let bin = resolve_youki()?;
        let bundle = bundle_dir(id)?;
        if !bundle.exists() {
            return Ok(());
        }
        let mode = resolve_run_mode()?;
        let saved = read_state(&bundle).await?;
        let had_state = saved.is_some();
        let mut order: Vec<String> = saved
            .map(|s| s.containers)
            .unwrap_or_default()
            .into_iter()
            .rev()
            .collect();
        order.push(id.to_string());
        for cid in &order {
            if let Err(e) = exec_youki(&bin, &mode, &["delete", cid], None).await {
                // A bundle with no state was never created in youki. A container
                // youki has already forgotten is gone too. Anything else stays on
                // disk so a later delete can retry.
                if had_state && !container_missing(&e) {
                    return Err(e);
                }
            }
        }
        remove_bundle(&bundle).await
    }

    async fn attach_volume(&self, req: &super::VolumeAttach) -> ApiResult<String> {
        let _guard = lock().lock().await;
        let bundle = bundle_dir(&req.machine)?;
        if !bundle.join("config.json").exists() && read_state(&bundle).await?.is_none() {
            return Err(ApiError::not_found(format!(
                "youki runtime: machine {} is not on this host",
                req.machine
            )));
        }
        host_path_ok(&req.path)?;
        let mut disks = read_disks(&bundle)?;
        let vol = req.volume.to_string();
        let device = if let Some(name) = req.device.as_deref() {
            check_device_name(name)?;
            name.to_owned()
        } else if let Some(existing) = disks.iter().find(|d| d.volume == vol) {
            existing.device.clone()
        } else {
            next_device(&disks)?
        };
        if let Some(slot) = disks.iter_mut().find(|d| d.volume == vol) {
            slot.path = req.path.clone();
            slot.device = device.clone();
            slot.read_only = req.read_only;
        } else {
            disks.push(AttachedDisk {
                volume: vol,
                path: req.path.clone(),
                device: device.clone(),
                read_only: req.read_only,
            });
        }
        write_disks(&bundle, &disks)?;
        refresh_disk_mounts(&bundle)?;
        Ok(device)
    }

    async fn detach_volume(&self, req: &super::VolumeDetach) -> ApiResult<()> {
        let _guard = lock().lock().await;
        let bundle = bundle_dir(&req.machine)?;
        if !bundle.join("config.json").exists() && read_state(&bundle).await?.is_none() {
            return Err(ApiError::not_found(format!(
                "youki runtime: machine {} is not on this host",
                req.machine
            )));
        }
        let vol = req.volume.to_string();
        let mut disks = read_disks(&bundle)?;
        let before = disks.len();
        disks.retain(|d| d.volume != vol);
        if disks.len() != before {
            write_disks(&bundle, &disks)?;
            refresh_disk_mounts(&bundle)?;
        }
        Ok(())
    }

    async fn list(&self) -> ApiResult<Vec<RunningMachine>> {
        let _guard = lock().lock().await;
        let _bin = resolve_youki()?;
        let root = state_dir();
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut rd = tokio::fs::read_dir(&root).await.map_err(|e| {
            ApiError::unavailable(format!("youki runtime: reading {}: {e}", root.display()))
        })?;
        let mut out = Vec::new();
        while let Some(ent) = rd.next_entry().await.map_err(|e| {
            ApiError::unavailable(format!("youki runtime: reading {}: {e}", root.display()))
        })? {
            let ft = ent.file_type().await.map_err(|e| {
                ApiError::unavailable(format!("youki runtime: reading {}: {e}", root.display()))
            })?;
            // Skip symlinks. file_type does not follow them.
            if !ft.is_dir() {
                continue;
            }
            let Some(saved) = read_state(&ent.path()).await? else {
                continue;
            };
            out.push(RunningMachine {
                id: parse_id(&saved.id)?,
                addresses: saved.addresses,
                power: saved.power,
            });
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    async fn container_probes(&self, id: &Id) -> Vec<crate::probe::ContainerProbes> {
        let Ok(bundle) = bundle_dir(id) else {
            return Vec::new();
        };
        let _guard = lock().lock().await;
        match read_state(&bundle).await {
            Ok(Some(saved)) => saved.probes,
            _ => Vec::new(),
        }
    }

    /// Command probe. `env` is empty and `cwd` is unset, so the container
    /// keeps its own environment and working directory. Those values are
    /// not read and not logged. Privilege follows `exec`: `sudo -n` when
    /// `PISTACK_YOUKI_SUDO` is set, and the rootless fallback otherwise.
    async fn command_probe(
        &self,
        id: &Id,
        container: &str,
        argv: &[String],
        timeout: Duration,
    ) -> ApiResult<ExecOutput> {
        self.exec(
            id,
            container,
            argv,
            &[],
            None,
            timeout,
            crate::probe::PROBE_EXEC_OUTPUT_CAP,
        )
        .await
    }
}

fn lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Tests that point youki at a fake binary share this lock. `PISTACK_YOUKI_BIN`
/// and `PISTACK_YOUKI_STATE` are process-wide.
#[cfg(test)]
pub(crate) fn youki_env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

impl YoukiRuntime {
    #[allow(clippy::too_many_arguments)]
    /// Run `argv` with `youki exec` inside `container` of the pod sandbox `id`.
    ///
    /// The invocation follows the same privilege path as `create` and `start`:
    /// `sudo -n <youki> exec ...` when sudo mode is on, and `youki` directly
    /// when the agent is root or in the rootless fallback. `env` and `cwd`
    /// are optional: an empty slice and `None` leave the container's own
    /// environment and working directory. `timeout` bounds the wait.
    /// `output_cap` is the most bytes kept of stdout and of stderr,
    /// separately. The returned exit code is the command's, including
    /// non-zero. A missing youki binary is unavailable. A container this
    /// sandbox does not have, or a machine that is not on this host, is not
    /// found. Stdin is closed. This does not allocate a terminal.
    pub async fn exec(
        &self,
        id: &Id,
        container: &str,
        argv: &[String],
        env: &[(String, String)],
        cwd: Option<&str>,
        timeout: Duration,
        output_cap: usize,
    ) -> ApiResult<ExecOutput> {
        if timeout.is_zero() {
            return Err(ApiError::bad_request(
                "youki runtime: exec timeout must be greater than zero",
            ));
        }
        validate_exec(argv, env, cwd)?;
        let bin = resolve_youki()?;
        let container_id = {
            let _guard = lock().lock().await;
            let bundle = bundle_dir(id)?;
            let Some(saved) = read_state(&bundle).await? else {
                return Err(ApiError::not_found(format!(
                    "youki runtime: machine {id} is not on this host"
                )));
            };
            if saved.power != PowerState::On {
                return Err(ApiError::unavailable(
                    "youki runtime: pod sandbox is not running",
                ));
            }
            resolve_container(&saved, container)?.to_owned()
        };
        let mode = resolve_run_mode()?;
        run_exec(
            &bin,
            &mode.privilege,
            &container_id,
            argv,
            env,
            cwd,
            timeout,
            output_cap,
        )
        .await
    }
}

fn state_dir() -> PathBuf {
    match std::env::var_os("PISTACK_YOUKI_STATE") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(DEFAULT_STATE),
    }
}

fn bundle_dir(id: &Id) -> ApiResult<PathBuf> {
    let name = id.to_string();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(ApiError::bad_request(format!(
            "youki runtime: machine id {name} cannot name a bundle"
        )));
    }
    Ok(state_dir().join(name))
}

/// `$PISTACK_YOUKI_BIN` if it is set (even when that path does not exist),
/// otherwise an executable `youki` on `PATH`.
fn resolve_youki() -> ApiResult<PathBuf> {
    if let Some(v) = std::env::var_os("PISTACK_YOUKI_BIN") {
        if !v.is_empty() {
            let path = PathBuf::from(&v);
            if is_executable(&path) {
                return Ok(path);
            }
            return Err(ApiError::unavailable(format!(
                "youki runtime: PISTACK_YOUKI_BIN {} is not available",
                path.display()
            )));
        }
    }
    crate::facts::which("youki")
        .ok_or_else(|| ApiError::unavailable("youki runtime: youki is not on PATH"))
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

/// How this agent is allowed to run youki.
#[derive(Clone, Debug)]
enum Privilege {
    /// Agent euid is 0. youki is executed directly with the root OCI config.
    Root,
    /// Agent is not root and `PISTACK_YOUKI_SUDO` is set. youki is
    /// `sudo -n <youki>` and the OCI config stays the root config.
    Sudo { sudo: PathBuf },
    /// Agent is not root and sudo mode is off. The OCI config creates a
    /// user namespace with a single-id map. No `newuidmap` binary is required.
    Rootless { uid: u32, gid: u32 },
}

/// Cgroup controllers this process may write. Root and sudo always report
/// both, because youki itself runs as root. Rootless reports a controller
/// only when the current cgroup directory is writable and lists it.
#[derive(Clone, Copy, Debug)]
struct CgroupCaps {
    memory: bool,
    cpu: bool,
}

#[derive(Clone, Debug)]
struct RunMode {
    privilege: Privilege,
    cgroup: CgroupCaps,
}

impl RunMode {
    #[cfg(test)]
    fn privileged() -> Self {
        Self {
            privilege: Privilege::Root,
            cgroup: CgroupCaps {
                memory: true,
                cpu: true,
            },
        }
    }

    #[cfg(test)]
    fn rootless(uid: u32, gid: u32) -> Self {
        Self {
            privilege: Privilege::Rootless { uid, gid },
            cgroup: CgroupCaps {
                memory: false,
                cpu: false,
            },
        }
    }
}

const SUDO_ENV: &str = "PISTACK_YOUKI_SUDO";
const SUDO_BIN_ENV: &str = "PISTACK_YOUKI_SUDO_BIN";
const NETNS_ENV: &str = "PISTACK_YOUKI_NETNS";

/// Sample sudoers rule. Operators replace the user and the youki path.
const SUDOERS_SAMPLE: &str = "ubuntu ALL=(root) NOPASSWD: /usr/bin/youki";

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn resolve_run_mode() -> ApiResult<RunMode> {
    let privilege = resolve_privilege()?;
    let cgroup = match privilege {
        Privilege::Rootless { .. } => probe_cgroup_caps(),
        Privilege::Root | Privilege::Sudo { .. } => CgroupCaps {
            memory: true,
            cpu: true,
        },
    };
    Ok(RunMode { privilege, cgroup })
}

fn resolve_privilege() -> ApiResult<Privilege> {
    let (uid, gid) = agent_ids();
    if uid == 0 {
        return Ok(Privilege::Root);
    }
    if env_flag(SUDO_ENV) {
        return match sudo_bin() {
            Some(sudo) => Ok(Privilege::Sudo { sudo }),
            None if unprivileged_userns_available() => Err(ApiError::unavailable(format!(
                "youki runtime: {SUDO_ENV} is set but sudo is not available. Install sudo and a NOPASSWD sudoers rule for the youki binary (`{SUDOERS_SAMPLE}`), or unset {SUDO_ENV} to use a rootless user namespace"
            ))),
            None => Err(privilege_unavailable()),
        };
    }
    if unprivileged_userns_available() {
        return Ok(Privilege::Rootless { uid, gid });
    }
    Err(privilege_unavailable())
}

fn privilege_unavailable() -> ApiError {
    ApiError::unavailable(format!(
        "youki runtime: the agent is not root, so youki needs {SUDO_ENV}=1 with a NOPASSWD sudoers rule for the youki binary (`{SUDOERS_SAMPLE}`) or an unprivileged user namespace for a single-id map (no newuidmap). Run the agent as root, set {SUDO_ENV}=1, or enable unprivileged user namespaces"
    ))
}

fn agent_ids() -> (u32, u32) {
    #[cfg(test)]
    if let Some(ids) = test_agent_ids() {
        return ids;
    }
    #[cfg(unix)]
    {
        // geteuid and getegid do not fail.
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        (uid, gid)
    }
    #[cfg(not(unix))]
    (0, 0)
}

#[cfg(test)]
fn test_agent_ids() -> Option<(u32, u32)> {
    let uid = std::env::var("PISTACK_YOUKI_TEST_EUID")
        .ok()
        .and_then(|v| v.parse().ok())?;
    let gid = std::env::var("PISTACK_YOUKI_TEST_EGID")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(uid);
    Some((uid, gid))
}

/// `PISTACK_YOUKI_SUDO_BIN` replaces `sudo` for tests. An empty value is ignored.
fn sudo_bin() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os(SUDO_BIN_ENV) {
        if !v.is_empty() {
            let path = PathBuf::from(v);
            return is_executable(&path).then_some(path);
        }
    }
    crate::facts::which("sudo")
}

fn unprivileged_userns_available() -> bool {
    #[cfg(test)]
    if let Ok(value) = std::env::var("PISTACK_YOUKI_TEST_USERNS") {
        return matches!(value.as_str(), "1" | "true" | "yes");
    }
    !sysctl_is_zero("/proc/sys/kernel/unprivileged_userns_clone")
        && !sysctl_is_zero("/proc/sys/user/max_user_namespaces")
}

fn sysctl_is_zero(path: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|text| text.trim() == "0")
        .unwrap_or(false)
}

fn probe_cgroup_caps() -> CgroupCaps {
    #[cfg(target_os = "linux")]
    {
        let Some(dir) = cgroup_v2_dir() else {
            return CgroupCaps {
                memory: false,
                cpu: false,
            };
        };
        if !dir.is_dir() || !dir_writable(&dir) {
            return CgroupCaps {
                memory: false,
                cpu: false,
            };
        }
        let Ok(text) = std::fs::read_to_string(dir.join("cgroup.controllers")) else {
            return CgroupCaps {
                memory: false,
                cpu: false,
            };
        };
        let has = |name: &str| text.split_whitespace().any(|c| c == name);
        CgroupCaps {
            memory: has("memory"),
            cpu: has("cpu"),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        CgroupCaps {
            memory: false,
            cpu: false,
        }
    }
}

#[cfg(target_os = "linux")]
fn cgroup_v2_dir() -> Option<PathBuf> {
    let text = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    for line in text.lines() {
        let mut parts = line.splitn(3, ':');
        let hierarchy = parts.next()?;
        let controllers = parts.next()?;
        let path = parts.next()?;
        // cgroup v2 is hierarchy 0 with an empty controller field.
        if hierarchy == "0" && controllers.is_empty() {
            return Some(Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/')));
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn dir_writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    // access follows the caller's credentials. W_OK is the write bit.
    unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 }
}

fn file_uid(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(path).ok().map(|meta| meta.uid())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Absolute `PISTACK_YOUKI_NETNS`, or nothing. Relative paths and `..` are ignored.
fn requested_host_netns() -> Option<PathBuf> {
    host_netns_from(std::env::var_os(NETNS_ENV).as_deref())
}

fn host_netns_from(raw: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let raw = raw?;
    if raw.is_empty() {
        return None;
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() || has_parent_dir(&path) {
        tracing::warn!(
            path = %path.display(),
            "youki runtime: ignoring {NETNS_ENV}; it must be an absolute path"
        );
        return None;
    }
    Some(path)
}

fn nic_addresses(req: &MachineRequest) -> Vec<String> {
    req.nics
        .iter()
        .map(|n| n.address.clone())
        .filter(|a| !a.is_empty())
        .collect()
}

fn parse_id(s: &str) -> ApiResult<Id> {
    Id::parse(kind::MACHINE, s).ok_or_else(|| {
        ApiError::unavailable(format!("youki runtime: state id {s} is not a machine id"))
    })
}

const MAX_IMAGE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
struct Launch {
    image: String,
    command: Vec<String>,
    args: Vec<String>,
    env: Vec<(String, String)>,
    cwd: String,
}

#[derive(Clone, Debug, Default)]
struct ImageConfig {
    entrypoint: Vec<String>,
    cmd: Vec<String>,
    env: Vec<String>,
    working_dir: String,
}

#[derive(Clone, Debug, Deserialize)]
struct SandboxDoc {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    containers: Vec<SandboxContainer>,
    #[serde(default)]
    volumes: Vec<SandboxVolume>,
    #[serde(default)]
    config_maps: BTreeMap<String, Projected>,
    #[serde(default)]
    secrets: BTreeMap<String, Projected>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SandboxContainer {
    #[serde(default)]
    name: String,
    #[serde(default)]
    image: String,
    #[serde(default)]
    command: Vec<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: Vec<SandboxEnv>,
    #[serde(default)]
    working_dir: String,
    #[serde(default)]
    volume_mounts: Vec<SandboxMount>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct SandboxEnv {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    value_from: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SandboxMount {
    #[serde(default)]
    name: String,
    #[serde(default)]
    mount_path: String,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    sub_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SandboxVolume {
    name: String,
    source: VolumeSrc,
}

/// Same JSON shape as loom's volume source: `"empty_dir"`,
/// `{"config_map":"<name>"}`, `{"secret":"<name>"}`, `{"volume":"<id>"}`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum VolumeSrc {
    EmptyDir,
    Secret(String),
    ConfigMap(String),
    Volume(String),
}

/// File bodies carried on the sandbox document. `binary_data` is base64.
#[derive(Clone, Debug, Default, Deserialize)]
struct Projected {
    #[serde(default)]
    data: BTreeMap<String, String>,
    #[serde(default)]
    binary_data: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
struct PodPlan {
    containers: Vec<SandboxContainer>,
    volumes: Vec<SandboxVolume>,
    config_maps: BTreeMap<String, Projected>,
    secrets: BTreeMap<String, Projected>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AttachedDisk {
    volume: String,
    path: String,
    device: String,
    #[serde(default)]
    read_only: bool,
}

struct PreparedVolume {
    host_path: PathBuf,
    directory: bool,
    force_read_only: bool,
}

struct OciBind {
    destination: String,
    source: String,
    read_only: bool,
    directory: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct MountPlan {
    volumes: Vec<SandboxVolume>,
    containers: Vec<PlannedMounts>,
}

#[derive(Clone, Serialize, Deserialize)]
struct PlannedMounts {
    index: usize,
    mounts: Vec<SandboxMount>,
}

#[derive(Clone, Copy)]
struct Limits {
    vcpus: u32,
    memory_mib: u64,
}

fn cannot_unpack() -> ApiError {
    ApiError::unavailable("youki runtime: image cannot be unpacked")
}

/// Pod sandbox JSON is the only `user_data` this runtime reads. Anything
/// else, including a raw secret, is ignored and never written down.
fn launch_from(req: &MachineRequest) -> Option<Launch> {
    let plan = sandbox_plan(req).ok()??;
    let env = plan
        .containers
        .first()
        .map(|c| resolve_env_pairs(&c.env, &plan.config_maps, &plan.secrets))
        .unwrap_or_default();
    let c = plan.containers.into_iter().next().unwrap_or_default();
    Some(Launch {
        image: c.image,
        command: c.command,
        args: c.args,
        env,
        cwd: c.working_dir,
    })
}

fn sandbox_plan(req: &MachineRequest) -> ApiResult<Option<PodPlan>> {
    let Some(raw) = req.user_data.as_deref() else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if value.get("kind").and_then(|k| k.as_str()) != Some("PodSandbox") {
        return Ok(None);
    }
    let doc: SandboxDoc = serde_json::from_value(value)
        .map_err(|_| ApiError::unavailable("youki runtime: sandbox document is invalid"))?;
    if doc.kind != "PodSandbox" {
        return Ok(None);
    }
    Ok(Some(PodPlan {
        containers: doc.containers,
        volumes: doc.volumes,
        config_maps: doc.config_maps,
        secrets: doc.secrets,
    }))
}

fn choose_image(req: &MachineRequest, launch: Option<&Launch>) -> ApiResult<PathBuf> {
    if let Some(launch) = launch {
        if let Some(path) = absolute_image(&launch.image) {
            return Ok(path);
        }
    }
    if let Some(path) = absolute_image(&req.image).or_else(|| digest_image(&req.image)) {
        return Ok(path);
    }
    if let Some(launch) = launch {
        if let Some(path) = digest_image(&launch.image) {
            return Ok(path);
        }
    }
    Err(cannot_unpack())
}

fn absolute_image(image: &str) -> Option<PathBuf> {
    let path = Path::new(image);
    if !path.is_absolute() || has_parent_dir(path) {
        return None;
    }
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        return None;
    }
    if meta.is_file() || meta.is_dir() {
        Some(path.to_path_buf())
    } else {
        None
    }
}

fn mold_dir() -> PathBuf {
    if let Some(v) = std::env::var_os("PISTACK_MOLD_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Some(v) = std::env::var_os("PISTACK_DATA_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v).join("images");
        }
    }
    PathBuf::from("data/images")
}

fn digest_image(image: &str) -> Option<PathBuf> {
    let hex = image.strip_prefix("sha256:")?;
    if hex.len() < 2 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let hex = hex.to_ascii_lowercase();
    let base = mold_dir().join("blobs").join("sha256");
    if has_parent_dir(&base) {
        return None;
    }
    for path in [base.join(&hex[..2]).join(&hex), base.join(&hex)] {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_file() || meta.is_dir() {
            return Some(path);
        }
    }
    None
}

fn has_parent_dir(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
        || path.to_string_lossy().split('/').any(|p| p == "..")
}

fn sandbox_namespaces(mode: &RunMode, host_netns: Option<&Path>) -> Vec<serde_json::Value> {
    let mut namespaces = Vec::new();
    if matches!(&mode.privilege, Privilege::Rootless { .. }) {
        // Created here, not joined. Mappings belong on this spec only.
        namespaces.push(serde_json::json!({ "type": "user" }));
    }
    namespaces.push(serde_json::json!({ "type": "pid" }));
    namespaces.push(network_namespace(mode, host_netns));
    for kind in ["ipc", "uts", "mount"] {
        namespaces.push(serde_json::json!({ "type": kind }));
    }
    namespaces
}

fn network_namespace(mode: &RunMode, host_netns: Option<&Path>) -> serde_json::Value {
    let Some(path) = host_netns else {
        return serde_json::json!({ "type": "network" });
    };
    let path_str = path.to_string_lossy().into_owned();
    match &mode.privilege {
        Privilege::Rootless { uid, .. } => match file_uid(path) {
            Some(owner) if owner == *uid => {
                serde_json::json!({ "type": "network", "path": path_str })
            }
            Some(owner) => {
                tracing::warn!(
                    path = %path.display(),
                    owner,
                    agent = uid,
                    "youki runtime: skipping network namespace owned by another uid; using the sandbox network namespace"
                );
                serde_json::json!({ "type": "network" })
            }
            None => {
                tracing::warn!(
                    path = %path.display(),
                    "youki runtime: skipping network namespace that cannot be inspected; using the sandbox network namespace"
                );
                serde_json::json!({ "type": "network" })
            }
        },
        Privilege::Root | Privilege::Sudo { .. } => {
            serde_json::json!({ "type": "network", "path": path_str })
        }
    }
}

fn container_namespaces(pid: u32, rootless: bool) -> Vec<serde_json::Value> {
    let join = |kind: &str, name: &str| {
        serde_json::json!({
            "type": kind,
            "path": format!("/proc/{pid}/ns/{name}"),
        })
    };
    let mut namespaces = Vec::new();
    // The user namespace is joined before the other shared namespaces.
    if rootless {
        namespaces.push(join("user", "user"));
    }
    namespaces.push(join("pid", "pid"));
    namespaces.push(join("network", "net"));
    namespaces.push(join("ipc", "ipc"));
    namespaces.push(join("uts", "uts"));
    namespaces.push(serde_json::json!({ "type": "mount" }));
    namespaces
}

impl RunMode {
    fn oci_config(
        &self,
        hostname: Option<&str>,
        limits: &Limits,
        process: (&[String], &[String], &str),
        namespaces: &[serde_json::Value],
        binds: &[OciBind],
    ) -> serde_json::Value {
        let (args, env, cwd) = process;
        let mut linux = serde_json::json!({ "namespaces": namespaces });
        if let Privilege::Rootless { uid, gid } = &self.privilege {
            if creates_user_namespace(namespaces) {
                linux["uidMappings"] = id_map(*uid);
                linux["gidMappings"] = id_map(*gid);
            }
        }
        let mut resources = serde_json::Map::new();
        if self.cgroup.memory && limits.memory_mib > 0 {
            let bytes = limits.memory_mib.saturating_mul(1024 * 1024);
            resources.insert("memory".into(), serde_json::json!({ "limit": bytes }));
        }
        if self.cgroup.cpu && limits.vcpus > 0 {
            let period: u64 = 100_000;
            let quota = period.saturating_mul(u64::from(limits.vcpus));
            resources.insert(
                "cpu".into(),
                serde_json::json!({ "period": period, "quota": quota }),
            );
        }
        if !resources.is_empty() {
            linux["resources"] = serde_json::Value::Object(resources);
        }
        let mut mounts = default_mounts();
        for bind in binds {
            mounts.push(bind_mount(bind));
        }
        let mut spec = serde_json::json!({
            "ociVersion": "1.0.2",
            "process": {
                "terminal": false,
                "user": {"uid": 0, "gid": 0},
                "args": args,
                "env": env,
                "cwd": cwd
            },
            "root": {"path": "rootfs", "readonly": false},
            "mounts": mounts,
            "linux": linux
        });
        if let Some(host) = hostname {
            spec["hostname"] = serde_json::Value::String(host.to_owned());
        }
        spec
    }
}

fn creates_user_namespace(namespaces: &[serde_json::Value]) -> bool {
    namespaces.iter().any(|ns| {
        ns.get("type").and_then(|kind| kind.as_str()) == Some("user")
            && namespace_path(ns).is_none()
    })
}

fn namespace_path(ns: &serde_json::Value) -> Option<&str> {
    ns.get("path")
        .and_then(|path| path.as_str())
        .filter(|path| !path.is_empty())
}

/// One map entry: container id 0 is the agent's id, size 1. youki writes that
/// directly and does not look up `newuidmap`.
fn id_map(host_id: u32) -> serde_json::Value {
    serde_json::json!([{ "containerID": 0, "hostID": host_id, "size": 1 }])
}

fn default_mounts() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"destination": "/proc", "type": "proc", "source": "proc"}),
        serde_json::json!({
            "destination": "/dev",
            "type": "tmpfs",
            "source": "tmpfs",
            "options": ["nosuid", "strictatime", "mode=755", "size=65536k"]
        }),
        serde_json::json!({
            "destination": "/sys",
            "type": "sysfs",
            "source": "sysfs",
            "options": ["nosuid", "noexec", "nodev", "ro"]
        }),
    ]
}

fn bind_mount(bind: &OciBind) -> serde_json::Value {
    let kind = if bind.directory { "rbind" } else { "bind" };
    let flag = if bind.read_only { "ro" } else { "rw" };
    serde_json::json!({
        "destination": bind.destination,
        "type": "bind",
        "source": bind.source,
        "options": [kind, "rprivate", flag],
    })
}

fn process_spec(launch: Option<&Launch>, cfg: &ImageConfig) -> (Vec<String>, Vec<String>, String) {
    let (cmd, args) = match launch {
        Some(l) if !l.command.is_empty() && !l.args.is_empty() => {
            (l.command.clone(), l.args.clone())
        }
        Some(l) if !l.command.is_empty() => (l.command.clone(), Vec::new()),
        Some(l) if !l.args.is_empty() => (cfg.entrypoint.clone(), l.args.clone()),
        _ => (cfg.entrypoint.clone(), cfg.cmd.clone()),
    };
    let mut proc = cmd;
    proc.extend(args);
    if proc.is_empty() {
        proc.push("/bin/sh".into());
    }
    let mut env_map = std::collections::BTreeMap::new();
    env_map.insert("PATH".into(), DEFAULT_PATH.into());
    for entry in &cfg.env {
        if let Some((k, v)) = entry.split_once('=') {
            if !k.is_empty() {
                env_map.insert(k.to_owned(), v.to_owned());
            }
        }
    }
    if let Some(l) = launch {
        for (k, v) in &l.env {
            env_map.insert(k.clone(), v.clone());
        }
    }
    let env = env_map
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    let cwd = launch
        .filter(|l| !l.cwd.is_empty())
        .map(|l| l.cwd.clone())
        .or_else(|| {
            if cfg.working_dir.is_empty() {
                None
            } else {
                Some(cfg.working_dir.clone())
            }
        })
        .unwrap_or_else(|| "/".into());
    (proc, env, cwd)
}

async fn write_bundle(bundle: &Path, req: &MachineRequest, mode: &RunMode) -> ApiResult<()> {
    let launch = launch_from(req);
    let image = choose_image(req, launch.as_ref())?;
    let limits = Limits {
        vcpus: req.resources.vcpus,
        memory_mib: req.resources.memory_mib,
    };
    let hostname = if req.name.is_empty() {
        req.id.to_string()
    } else {
        req.name.clone()
    };
    let bundle = bundle.to_path_buf();
    let mode = mode.clone();
    tokio::task::spawn_blocking(move || {
        install_bundle(&bundle, &image, &hostname, &limits, launch.as_ref(), &mode)
    })
    .await
    .map_err(|_| ApiError::unavailable("youki runtime: image unpack stopped"))?
}

fn install_bundle(
    bundle: &Path,
    image: &Path,
    hostname: &str,
    limits: &Limits,
    launch: Option<&Launch>,
    mode: &RunMode,
) -> ApiResult<()> {
    std::fs::create_dir_all(bundle).map_err(|e| {
        ApiError::unavailable(format!("youki runtime: creating {}: {e}", bundle.display()))
    })?;
    let rootfs = bundle.join("rootfs");
    reset_dir(&rootfs)?;
    let cfg = unpack_image(image, &rootfs)?;
    if rootfs_empty(&rootfs) {
        return Err(ApiError::unavailable(
            "youki runtime: image rootfs is empty",
        ));
    }
    let image_note = bundle.join("image");
    if std::fs::symlink_metadata(&image_note).is_ok() {
        let _ = std::fs::remove_file(&image_note);
    }
    let (args, env, cwd) = process_spec(launch, &cfg);
    let netns = requested_host_netns();
    let text = serde_json::to_vec_pretty(&mode.oci_config(
        Some(hostname),
        limits,
        (&args, &env, &cwd),
        &sandbox_namespaces(mode, netns.as_deref()),
        &[],
    ))
    .map_err(|e| ApiError::internal(format!("youki runtime: encoding config: {e}")))?;
    std::fs::write(bundle.join("config.json"), text)
        .map_err(|e| ApiError::unavailable(format!("youki runtime: writing config.json: {e}")))
}

fn rootfs_empty(rootfs: &Path) -> bool {
    std::fs::read_dir(rootfs)
        .map(|mut rd| rd.next().is_none())
        .unwrap_or(true)
}

fn reset_dir(dir: &Path) -> ApiResult<()> {
    if let Ok(meta) = std::fs::symlink_metadata(dir) {
        let ft = meta.file_type();
        let removed = if ft.is_symlink() || !ft.is_dir() {
            std::fs::remove_file(dir)
        } else {
            std::fs::remove_dir_all(dir)
        };
        removed.map_err(|e| {
            ApiError::unavailable(format!("youki runtime: removing {}: {e}", dir.display()))
        })?;
    }
    std::fs::create_dir_all(dir).map_err(|e| {
        ApiError::unavailable(format!("youki runtime: creating {}: {e}", dir.display()))
    })
}

fn unpack_image(src: &Path, rootfs: &Path) -> ApiResult<ImageConfig> {
    let meta = std::fs::symlink_metadata(src).map_err(|_| cannot_unpack())?;
    if meta.file_type().is_symlink() {
        return Err(cannot_unpack());
    }
    if meta.is_dir() {
        if src.join("oci-layout").is_file() {
            return apply_oci(src, rootfs);
        }
        copy_tree(src, rootfs)?;
        return Ok(ImageConfig::default());
    }
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_IMAGE_BYTES {
        return Err(cannot_unpack());
    }
    unpack_archive(src, rootfs)
}

fn unpack_archive(src: &Path, rootfs: &Path) -> ApiResult<ImageConfig> {
    let owned = decompress_to_tar(src)?;
    let staging = temp_path("stage");
    let _stage = TempPath(staging.clone());
    std::fs::create_dir_all(&staging).map_err(|_| cannot_unpack())?;
    extract_tar(&owned.path, &staging, false)?;
    if docker_manifest(&staging) {
        apply_docker(&staging, rootfs)
    } else if staging.join("oci-layout").is_file() && staging.join("index.json").is_file() {
        apply_oci(&staging, rootfs)
    } else {
        reset_dir(rootfs)?;
        extract_tar(&owned.path, rootfs, true)?;
        Ok(ImageConfig::default())
    }
}

struct TempPath(PathBuf);
impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct OwnedTar {
    path: PathBuf,
    temporary: bool,
}
impl Drop for OwnedTar {
    fn drop(&mut self) {
        if self.temporary {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn decompress_to_tar(src: &Path) -> ApiResult<OwnedTar> {
    let mut file = std::fs::File::open(src).map_err(|_| cannot_unpack())?;
    let mut magic = [0u8; 4];
    let n = file.read(&mut magic).unwrap_or(0);
    if n >= 4 && magic == [0x28, 0xB5, 0x2F, 0xFD] {
        return Err(ApiError::unavailable(
            "youki runtime: image layer compression is not available",
        ));
    }
    if n >= 2 && magic[0] == 0x1f && magic[1] == 0x8b {
        let dst = temp_path("tar");
        gunzip(src, &dst)?;
        return Ok(OwnedTar {
            path: dst,
            temporary: true,
        });
    }
    Ok(OwnedTar {
        path: src.to_path_buf(),
        temporary: false,
    })
}

fn gunzip(src: &Path, dst: &Path) -> ApiResult<()> {
    let out = std::fs::File::create(dst)
        .map_err(|_| ApiError::unavailable("youki runtime: gzip is not available"))?;
    match std::process::Command::new("gzip")
        .arg("-dc")
        .arg(src)
        .stdout(out)
        .stderr(Stdio::null())
        .status()
    {
        Err(_) => Err(ApiError::unavailable(
            "youki runtime: gzip is not available",
        )),
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(cannot_unpack()),
    }
}

fn temp_path(suffix: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "yk-{}-{}-{suffix}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

fn docker_manifest(staging: &Path) -> bool {
    let Ok(bytes) = std::fs::read(staging.join("manifest.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    value.as_array().and_then(|a| a.first()).is_some_and(|e| {
        e.get("Layers").is_some() || e.get("layers").is_some() || e.get("Config").is_some()
    })
}

fn apply_docker(staging: &Path, rootfs: &Path) -> ApiResult<ImageConfig> {
    let bytes = std::fs::read(staging.join("manifest.json")).map_err(|_| cannot_unpack())?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| cannot_unpack())?;
    let entry = value
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(cannot_unpack)?;
    let cfg = match entry.get("Config").and_then(|v| v.as_str()) {
        Some(path) => read_config_file(&staging.join(safe_rel(path)?))?,
        None => ImageConfig::default(),
    };
    let layers = entry
        .get("Layers")
        .or_else(|| entry.get("layers"))
        .and_then(|v| v.as_array())
        .ok_or_else(cannot_unpack)?;
    let mut budget = MAX_IMAGE_BYTES;
    for layer in layers {
        let rel = layer.as_str().ok_or_else(cannot_unpack)?;
        apply_layer(&staging.join(safe_rel(rel)?), rootfs, &mut budget)?;
    }
    Ok(cfg)
}

fn apply_oci(layout: &Path, rootfs: &Path) -> ApiResult<ImageConfig> {
    let index = read_json(&layout.join("index.json"))?;
    let digest = index
        .get("manifests")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|m| m.get("digest"))
        .and_then(|d| d.as_str())
        .ok_or_else(cannot_unpack)?;
    let manifest = read_json(&blob_file(layout, digest)?)?;
    let cfg = match manifest
        .get("config")
        .and_then(|c| c.get("digest"))
        .and_then(|d| d.as_str())
    {
        Some(digest) => read_config_file(&blob_file(layout, digest)?)?,
        None => ImageConfig::default(),
    };
    let layers = manifest
        .get("layers")
        .and_then(|v| v.as_array())
        .ok_or_else(cannot_unpack)?;
    let mut budget = MAX_IMAGE_BYTES;
    for layer in layers {
        if layer
            .get("mediaType")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .contains("zstd")
        {
            return Err(ApiError::unavailable(
                "youki runtime: image layer compression is not available",
            ));
        }
        let digest = layer
            .get("digest")
            .and_then(|d| d.as_str())
            .ok_or_else(cannot_unpack)?;
        apply_layer(&blob_file(layout, digest)?, rootfs, &mut budget)?;
    }
    Ok(cfg)
}

fn read_json(path: &Path) -> ApiResult<serde_json::Value> {
    let bytes = std::fs::read(path).map_err(|_| cannot_unpack())?;
    serde_json::from_slice(&bytes).map_err(|_| cannot_unpack())
}

fn read_config_file(path: &Path) -> ApiResult<ImageConfig> {
    Ok(image_config(&read_json(path)?))
}

fn image_config(value: &serde_json::Value) -> ImageConfig {
    let cfg = value.get("config").unwrap_or(value);
    ImageConfig {
        entrypoint: json_strings(cfg, "Entrypoint", "entrypoint"),
        cmd: json_strings(cfg, "Cmd", "cmd"),
        env: json_strings(cfg, "Env", "env"),
        working_dir: json_string(cfg, "WorkingDir", "workingDir"),
    }
}

fn json_strings(value: &serde_json::Value, a: &str, b: &str) -> Vec<String> {
    match value.get(a).or_else(|| value.get(b)) {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

fn json_string(value: &serde_json::Value, a: &str, b: &str) -> String {
    value
        .get(a)
        .or_else(|| value.get(b))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned()
}

fn blob_file(layout: &Path, digest: &str) -> ApiResult<PathBuf> {
    let hex = digest.strip_prefix("sha256:").ok_or_else(cannot_unpack)?;
    if hex.len() < 2 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(cannot_unpack());
    }
    let hex = hex.to_ascii_lowercase();
    let base = layout.join("blobs").join("sha256");
    for path in [base.join(&hex), base.join(&hex[..2]).join(&hex)] {
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            if !meta.file_type().is_symlink() && meta.is_file() {
                return Ok(path);
            }
        }
    }
    Err(cannot_unpack())
}

fn safe_rel(name: &str) -> ApiResult<PathBuf> {
    rel_under(name).ok_or_else(cannot_unpack)
}

fn apply_layer(path: &Path, rootfs: &Path, budget: &mut u64) -> ApiResult<()> {
    let owned = decompress_to_tar(path)?;
    extract_tar_budget(&owned.path, rootfs, true, budget)
}

fn extract_tar(path: &Path, dest: &Path, whiteouts: bool) -> ApiResult<()> {
    let mut budget = MAX_IMAGE_BYTES;
    extract_tar_budget(path, dest, whiteouts, &mut budget)
}

fn extract_tar_budget(
    path: &Path,
    dest: &Path,
    whiteouts: bool,
    budget: &mut u64,
) -> ApiResult<()> {
    let file = std::fs::File::open(path).map_err(|_| cannot_unpack())?;
    let mut tar = TarReader {
        inner: file,
        pending_name: None,
        pending_link: None,
    };
    while let Some(ent) = tar.next_entry()? {
        let rel = match rel_under(&ent.name) {
            Some(rel) => rel,
            None if ent.name.is_empty() || ent.name == "." || ent.name == "./" => {
                tar.discard(ent.size)?;
                continue;
            }
            None => return Err(cannot_unpack()),
        };
        if whiteouts && apply_whiteout(dest, &rel)? {
            tar.discard(ent.size)?;
            continue;
        }
        match ent.flag {
            b'5' => {
                tar.discard(ent.size)?;
                std::fs::create_dir_all(dest.join(&rel)).map_err(|_| cannot_unpack())?;
            }
            b'2' => {
                tar.discard(ent.size)?;
                place_symlink(dest, &rel, &ent.link)?;
            }
            b'1' => {
                tar.discard(ent.size)?;
                place_hardlink(dest, &rel, &ent.link)?;
            }
            b'0' | 0 => {
                let dest_file = dest.join(&rel);
                if let Some(parent) = dest_file.parent() {
                    std::fs::create_dir_all(parent).map_err(|_| cannot_unpack())?;
                }
                remove_path(&dest_file)?;
                let out = std::fs::File::create(&dest_file).map_err(|_| cannot_unpack())?;
                tar.copy_entry(ent.size, out, budget)?;
            }
            _ => tar.discard(ent.size)?,
        }
    }
    Ok(())
}

fn apply_whiteout(root: &Path, rel: &Path) -> ApiResult<bool> {
    let Some(name) = rel.file_name().and_then(|s| s.to_str()) else {
        return Ok(false);
    };
    if name == ".wh..wh..opq" {
        let dir = root.join(rel.parent().unwrap_or(Path::new("")));
        clear_children(&dir)?;
        return Ok(true);
    }
    if let Some(rest) = name.strip_prefix(".wh.") {
        if rest.is_empty() || rest.contains('/') || rest == ".." {
            return Err(cannot_unpack());
        }
        let target = root.join(rel.parent().unwrap_or(Path::new(""))).join(rest);
        remove_path(&target)?;
        return Ok(true);
    }
    Ok(false)
}

fn clear_children(dir: &Path) -> ApiResult<()> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(cannot_unpack()),
    };
    for ent in rd {
        let ent = ent.map_err(|_| cannot_unpack())?;
        remove_path(&ent.path())?;
    }
    Ok(())
}

fn remove_path(path: &Path) -> ApiResult<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(cannot_unpack()),
    };
    let result = if meta.file_type().is_symlink() || !meta.is_dir() {
        std::fs::remove_file(path)
    } else {
        std::fs::remove_dir_all(path)
    };
    result.map_err(|_| cannot_unpack())
}

fn place_symlink(root: &Path, rel: &Path, target: &str) -> ApiResult<()> {
    if !symlink_stays(rel, target) {
        return Err(cannot_unpack());
    }
    let dest = root.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|_| cannot_unpack())?;
    }
    remove_path(&dest)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, &dest).map_err(|_| cannot_unpack())
    }
    #[cfg(not(unix))]
    {
        let _ = (dest, target);
        Err(cannot_unpack())
    }
}

fn symlink_stays(link_rel: &Path, target: &str) -> bool {
    let path = Path::new(target);
    if path.is_absolute() {
        return !has_parent_dir(path);
    }
    let mut depth = link_rel
        .parent()
        .unwrap_or(Path::new(""))
        .components()
        .filter(|c| matches!(c, Component::Normal(_)))
        .count() as i32;
    for c in path.components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

fn place_hardlink(root: &Path, rel: &Path, target: &str) -> ApiResult<()> {
    let target_rel = rel_under(target).ok_or_else(cannot_unpack)?;
    let dest = root.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|_| cannot_unpack())?;
    }
    remove_path(&dest)?;
    std::fs::hard_link(root.join(target_rel), dest).map_err(|_| cannot_unpack())
}

fn copy_tree(src: &Path, dst: &Path) -> ApiResult<()> {
    let meta = std::fs::symlink_metadata(src).map_err(|_| cannot_unpack())?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(cannot_unpack());
    }
    std::fs::create_dir_all(dst).map_err(|_| cannot_unpack())?;
    copy_children(src, dst)
}

fn copy_children(src: &Path, dst: &Path) -> ApiResult<()> {
    for ent in std::fs::read_dir(src).map_err(|_| cannot_unpack())? {
        let ent = ent.map_err(|_| cannot_unpack())?;
        let name = ent.file_name();
        let name_str = name.to_string_lossy();
        if name_str.is_empty() || name_str.contains('/') || name_str == ".." || name_str == "." {
            return Err(cannot_unpack());
        }
        let from = ent.path();
        let to = dst.join(&name);
        let meta = std::fs::symlink_metadata(&from).map_err(|_| cannot_unpack())?;
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&from).map_err(|_| cannot_unpack())?;
            let target = target.to_string_lossy().into_owned();
            let name = name.to_string_lossy().into_owned();
            place_symlink(dst, Path::new(&name), &target)?;
        } else if meta.is_dir() {
            std::fs::create_dir_all(&to).map_err(|_| cannot_unpack())?;
            copy_children(&from, &to)?;
        } else if meta.is_file() {
            copy_file(&from, &to)?;
        }
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path) -> ApiResult<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|_| cannot_unpack())?;
    }
    let mut input = std::fs::File::open(from).map_err(|_| cannot_unpack())?;
    let mut output = std::fs::File::create(to).map_err(|_| cannot_unpack())?;
    std::io::copy(&mut input, &mut output).map_err(|_| cannot_unpack())?;
    Ok(())
}

struct TarReader<R: Read> {
    inner: R,
    pending_name: Option<String>,
    pending_link: Option<String>,
}

struct TarEntry {
    name: String,
    link: String,
    size: u64,
    flag: u8,
}

impl<R: Read> TarReader<R> {
    fn next_entry(&mut self) -> ApiResult<Option<TarEntry>> {
        loop {
            let mut header = [0u8; 512];
            match self.inner.read_exact(&mut header) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(_) => return Err(cannot_unpack()),
            }
            if header.iter().all(|b| *b == 0) {
                return Ok(None);
            }
            if !checksum_ok(&header) {
                return Err(cannot_unpack());
            }
            let flag = header[156];
            let size = parse_number(&header[124..136]).ok_or_else(cannot_unpack)?;
            if size > MAX_IMAGE_BYTES {
                return Err(cannot_unpack());
            }
            let mut name = tar_name(&header);
            let mut link = cstr(&header[157..257]);
            match flag {
                b'L' => {
                    self.pending_name = Some(self.read_string(size)?);
                    continue;
                }
                b'K' => {
                    self.pending_link = Some(self.read_string(size)?);
                    continue;
                }
                b'x' | b'g' => {
                    let text = self.read_bytes(size)?;
                    let (path, linkpath) = parse_pax(&text);
                    if let Some(path) = path {
                        self.pending_name = Some(path);
                    }
                    if let Some(linkpath) = linkpath {
                        self.pending_link = Some(linkpath);
                    }
                    continue;
                }
                _ => {}
            }
            if let Some(over) = self.pending_name.take() {
                name = over;
            }
            if let Some(over) = self.pending_link.take() {
                link = over;
            }
            return Ok(Some(TarEntry {
                name,
                link,
                size,
                flag,
            }));
        }
    }

    fn read_string(&mut self, size: u64) -> ApiResult<String> {
        let bytes = self.read_bytes(size)?;
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    fn read_bytes(&mut self, size: u64) -> ApiResult<Vec<u8>> {
        if size > MAX_IMAGE_BYTES {
            return Err(cannot_unpack());
        }
        let mut buf = vec![0u8; size as usize];
        self.inner
            .read_exact(&mut buf)
            .map_err(|_| cannot_unpack())?;
        self.skip_pad(size)?;
        Ok(buf)
    }

    fn copy_entry(&mut self, size: u64, mut out: impl Write, budget: &mut u64) -> ApiResult<()> {
        if size > *budget {
            return Err(cannot_unpack());
        }
        *budget -= size;
        let mut left = size;
        let mut buf = [0u8; 8192];
        while left > 0 {
            let n = left.min(buf.len() as u64) as usize;
            self.inner
                .read_exact(&mut buf[..n])
                .map_err(|_| cannot_unpack())?;
            out.write_all(&buf[..n]).map_err(|_| cannot_unpack())?;
            left -= n as u64;
        }
        self.skip_pad(size)
    }

    fn discard(&mut self, size: u64) -> ApiResult<()> {
        let mut left = size;
        let mut buf = [0u8; 8192];
        while left > 0 {
            let n = left.min(buf.len() as u64) as usize;
            self.inner
                .read_exact(&mut buf[..n])
                .map_err(|_| cannot_unpack())?;
            left -= n as u64;
        }
        self.skip_pad(size)
    }

    fn skip_pad(&mut self, size: u64) -> ApiResult<()> {
        let pad = ((512 - (size % 512)) % 512) as usize;
        if pad == 0 {
            return Ok(());
        }
        let mut buf = vec![0u8; pad];
        self.inner
            .read_exact(&mut buf)
            .map_err(|_| cannot_unpack())?;
        Ok(())
    }
}

fn tar_name(header: &[u8]) -> String {
    let name = cstr(&header[0..100]);
    let magic = &header[257..262];
    let prefix = cstr(&header[345..500]);
    if magic == b"ustar" && !prefix.is_empty() {
        format!("{prefix}/{name}")
    } else {
        name
    }
}

fn cstr(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end])
        .trim_end_matches(' ')
        .to_owned()
}

fn checksum_ok(header: &[u8]) -> bool {
    let Some(stored) = parse_octal(&header[148..156]) else {
        return false;
    };
    let mut sum = 0u64;
    for (i, b) in header.iter().enumerate() {
        if (148..156).contains(&i) {
            sum += b' ' as u64;
        } else {
            sum += *b as u64;
        }
    }
    stored == sum
}

fn parse_number(bytes: &[u8]) -> Option<u64> {
    if bytes.first().is_some_and(|b| b & 0x80 != 0) {
        let mut n: u128 = 0;
        for (i, b) in bytes.iter().enumerate() {
            let v = if i == 0 { b & 0x7f } else { *b };
            n = (n << 8) | u128::from(v);
            if n > u128::from(u64::MAX) {
                return None;
            }
        }
        return Some(n as u64);
    }
    parse_octal(bytes)
}

fn parse_octal(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes).ok()?;
    let text = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if text.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(text, 8).ok()
}

fn parse_pax(data: &[u8]) -> (Option<String>, Option<String>) {
    let mut path = None;
    let mut link = None;
    let mut i = 0;
    while i < data.len() {
        let rest = &data[i..];
        let Some(sp) = rest.iter().position(|b| *b == b' ') else {
            break;
        };
        let Ok(len) = std::str::from_utf8(&rest[..sp])
            .unwrap_or("")
            .parse::<usize>()
        else {
            break;
        };
        if len <= sp || i + len > data.len() {
            break;
        }
        let rec = &data[i + sp + 1..i + len];
        let rec = rec.strip_suffix(b"\n").unwrap_or(rec);
        if let Ok(text) = std::str::from_utf8(rec) {
            if let Some((k, v)) = text.split_once('=') {
                if k == "path" {
                    path = Some(v.to_owned());
                } else if k == "linkpath" {
                    link = Some(v.to_owned());
                }
            }
        }
        i += len;
    }
    (path, link)
}

fn rel_under(name: &str) -> Option<PathBuf> {
    if name.contains('\0') {
        return None;
    }
    let mut out = PathBuf::new();
    for c in Path::new(name).components() {
        match c {
            Component::Normal(s) => {
                let text = s.to_string_lossy();
                if text.is_empty() || text == ".." || text.contains('/') {
                    return None;
                }
                out.push(s);
            }
            Component::CurDir | Component::RootDir => {}
            Component::ParentDir | Component::Prefix(_) => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

async fn remove_bundle(bundle: &Path) -> ApiResult<()> {
    let rootfs = bundle.join("rootfs");
    if let Ok(meta) = tokio::fs::symlink_metadata(&rootfs).await {
        if meta.file_type().is_symlink() {
            tokio::fs::remove_file(&rootfs).await.map_err(|e| {
                ApiError::unavailable(format!(
                    "youki runtime: unlinking {}: {e}",
                    rootfs.display()
                ))
            })?;
        }
    }
    match tokio::fs::remove_dir_all(bundle).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ApiError::unavailable(format!(
            "youki runtime: removing {}: {e}",
            bundle.display()
        ))),
    }
}

async fn read_state(bundle: &Path) -> ApiResult<Option<Saved>> {
    let path = bundle.join("state.json");
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let saved = serde_json::from_slice(&bytes).map_err(|e| {
                ApiError::unavailable(format!("youki runtime: state {}: {e}", path.display()))
            })?;
            Ok(Some(saved))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ApiError::unavailable(format!(
            "youki runtime: reading {}: {e}",
            path.display()
        ))),
    }
}

async fn write_state(bundle: &Path, saved: &Saved) -> ApiResult<()> {
    let path = bundle.join("state.json");
    let text = serde_json::to_vec_pretty(saved)
        .map_err(|e| ApiError::internal(format!("youki runtime: encoding state: {e}")))?;
    tokio::fs::write(&path, text).await.map_err(|e| {
        ApiError::unavailable(format!("youki runtime: writing {}: {e}", path.display()))
    })
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Static x86_64 program: sleep 3600s, then again. It is pid 1 of the sandbox
/// and is not executed by the tests.
const PAUSE_ELF: &[u8] = &[
    0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x02, 0x00, 0x3e, 0x00, 0x01, 0x00, 0x00, 0x00, 0x78, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x38, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x9a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x9a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x48, 0x8d, 0x3d, 0x0b, 0x00, 0x00, 0x00, 0x31,
    0xf6, 0xb8, 0x23, 0x00, 0x00, 0x00, 0x0f, 0x05, 0xeb, 0xee, 0x10, 0x0e, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EnvSource {
    ConfigMap { name: String, key: String },
    Secret { name: String, key: String },
}

struct PreparedProcess {
    args: Vec<String>,
    env: Vec<String>,
    cwd: String,
}

fn resolve_env_pairs(
    env: &[SandboxEnv],
    config_maps: &BTreeMap<String, Projected>,
    secrets: &BTreeMap<String, Projected>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in env {
        if entry.name.is_empty() {
            continue;
        }
        // A present value_from replaces the literal, even when the ref is unusable.
        if let Some(value) = entry.value_from.as_ref() {
            let Some(source) = parse_env_source(value) else {
                continue;
            };
            let Some(resolved) = lookup_env(&source, config_maps, secrets) else {
                continue;
            };
            out.push((entry.name.clone(), resolved));
        } else {
            out.push((entry.name.clone(), entry.value.clone()));
        }
    }
    out
}

fn parse_env_source(value: &serde_json::Value) -> Option<EnvSource> {
    let source: EnvSource = serde_json::from_value(value.clone()).ok()?;
    let (name, key) = match &source {
        EnvSource::ConfigMap { name, key } | EnvSource::Secret { name, key } => (name, key),
    };
    if name.is_empty() || key.is_empty() {
        None
    } else {
        Some(source)
    }
}

fn lookup_env(
    source: &EnvSource,
    config_maps: &BTreeMap<String, Projected>,
    secrets: &BTreeMap<String, Projected>,
) -> Option<String> {
    let (projected, key) = match source {
        EnvSource::ConfigMap { name, key } => (config_maps.get(name)?, key),
        EnvSource::Secret { name, key } => (secrets.get(name)?, key),
    };
    lookup_projected(projected, key)
}

fn lookup_projected(projected: &Projected, key: &str) -> Option<String> {
    if let Some(text) = projected.data.get(key) {
        return Some(text.clone());
    }
    let bytes = decode_b64(projected.binary_data.get(key)?)?;
    String::from_utf8(bytes).ok()
}

fn decode_b64(text: &str) -> Option<Vec<u8>> {
    use base64::{
        engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
        Engine,
    };
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return None;
    }
    STANDARD
        .decode(compact.as_bytes())
        .or_else(|_| STANDARD_NO_PAD.decode(compact.as_bytes()))
        .ok()
}

#[cfg(test)]
fn resolve_process_env(
    image_env: &[String],
    container_env: &[SandboxEnv],
    config_maps: &BTreeMap<String, Projected>,
    secrets: &BTreeMap<String, Projected>,
) -> Vec<String> {
    let launch = Launch {
        env: resolve_env_pairs(container_env, config_maps, secrets),
        ..Launch::default()
    };
    let cfg = ImageConfig {
        env: image_env.to_vec(),
        ..ImageConfig::default()
    };
    process_spec(Some(&launch), &cfg).1
}

fn container_youki_id(machine: &Id, index: usize) -> String {
    format!("{machine}-{index}")
}

fn container_dir(pod: &Path, index: usize) -> PathBuf {
    pod.join("containers").join(index.to_string())
}

fn volume_dir_name(index: usize, name: &str) -> String {
    if name.is_empty() || name == "." || name == ".." {
        return format!("{index}-vol");
    }
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let safe = if safe.is_empty() { "vol" } else { &safe };
    format!("{index}-{safe}")
}

fn volume_host_dir(pod: &Path, index: usize, name: &str) -> PathBuf {
    pod.join("volumes").join(volume_dir_name(index, name))
}

fn valid_destination(path: &str) -> bool {
    if path.len() < 2 || !path.starts_with('/') || path.contains('\0') {
        return false;
    }
    if path == "/proc" || path == "/dev" || path == "/sys" {
        return false;
    }
    !has_parent_dir(Path::new(path))
}

fn choose_container_image(index: usize, image: &str, fallback: &str) -> ApiResult<PathBuf> {
    if let Some(path) = absolute_image(image).or_else(|| digest_image(image)) {
        return Ok(path);
    }
    if index != 0 {
        return Err(cannot_unpack());
    }
    absolute_image(fallback)
        .or_else(|| digest_image(fallback))
        .ok_or_else(cannot_unpack)
}

fn cannot_unpack_container(container: &SandboxContainer, index: usize) -> ApiError {
    let who = if container.name.is_empty() {
        format!("container {index}")
    } else {
        format!("container {}", container.name)
    };
    ApiError::unavailable(format!("youki runtime: image for {who} cannot be unpacked"))
}

/// Sandbox bundle plus one bundle per container. Does not call youki.
/// Container configs join `pid`'s namespaces; the sandbox config does not.
#[cfg(test)]
impl RunMode {
    fn stage_pod_bundles(
        &self,
        bundle: &Path,
        hostname: &str,
        limits: &Limits,
        plan: &PodPlan,
        fallback_image: &str,
        pid: u32,
    ) -> ApiResult<()> {
        let processes = prepare_pod(bundle, hostname, limits, plan, fallback_image, self)?;
        write_container_configs(bundle, plan, &processes, &[], limits, pid, self)
    }
}

fn prepare_pod(
    bundle: &Path,
    hostname: &str,
    limits: &Limits,
    plan: &PodPlan,
    fallback_image: &str,
    mode: &RunMode,
) -> ApiResult<Vec<PreparedProcess>> {
    std::fs::create_dir_all(bundle).map_err(|e| volume_io(bundle, e))?;
    materialize_volumes(bundle, plan)?;
    write_mount_plan(bundle, plan)?;
    write_pause_rootfs(bundle, hostname, limits, mode)?;
    let mut processes = Vec::new();
    for (index, container) in plan.containers.iter().enumerate() {
        let image = choose_container_image(index, &container.image, fallback_image)
            .map_err(|_| cannot_unpack_container(container, index))?;
        let dir = container_dir(bundle, index);
        std::fs::create_dir_all(&dir).map_err(|e| volume_io(&dir, e))?;
        let rootfs = dir.join("rootfs");
        reset_dir(&rootfs)?;
        let cfg = unpack_image(&image, &rootfs)?;
        if rootfs_empty(&rootfs) {
            return Err(ApiError::unavailable(
                "youki runtime: image rootfs is empty",
            ));
        }
        let launch = Launch {
            image: container.image.clone(),
            command: container.command.clone(),
            args: container.args.clone(),
            env: resolve_env_pairs(&container.env, &plan.config_maps, &plan.secrets),
            cwd: container.working_dir.clone(),
        };
        let (args, env, cwd) = process_spec(Some(&launch), &cfg);
        processes.push(PreparedProcess { args, env, cwd });
    }
    Ok(processes)
}

fn write_pause_rootfs(
    bundle: &Path,
    hostname: &str,
    limits: &Limits,
    mode: &RunMode,
) -> ApiResult<()> {
    let rootfs = bundle.join("rootfs");
    reset_dir(&rootfs)?;
    write_mode(&rootfs.join("pause"), PAUSE_ELF, 0o755)?;
    let args = vec!["/pause".to_string()];
    let env = vec![format!("PATH={DEFAULT_PATH}")];
    let netns = requested_host_netns();
    let spec = mode.oci_config(
        Some(hostname),
        limits,
        (&args, &env, "/"),
        &sandbox_namespaces(mode, netns.as_deref()),
        &[],
    );
    write_json(&bundle.join("config.json"), &spec, 0o644)
}

fn write_container_configs(
    bundle: &Path,
    plan: &PodPlan,
    processes: &[PreparedProcess],
    disks: &[AttachedDisk],
    limits: &Limits,
    pid: u32,
    mode: &RunMode,
) -> ApiResult<()> {
    // Projected dirs are rewritten; EmptyDir and container rootfs are not.
    materialize_volumes(bundle, plan)?;
    let rootless = matches!(&mode.privilege, Privilege::Rootless { .. });
    let namespaces = container_namespaces(pid, rootless);
    for (index, process) in processes.iter().enumerate() {
        let mounts = plan
            .containers
            .get(index)
            .map(|container| container.volume_mounts.as_slice())
            .unwrap_or(&[]);
        let binds = oci_binds(bundle, &plan.volumes, mounts, disks);
        let spec = mode.oci_config(
            None,
            limits,
            (&process.args, &process.env, &process.cwd),
            &namespaces,
            &binds,
        );
        write_json(
            &container_dir(bundle, index).join("config.json"),
            &spec,
            0o600,
        )?;
    }
    Ok(())
}

fn write_mount_plan(pod: &Path, plan: &PodPlan) -> ApiResult<()> {
    let planned = MountPlan {
        volumes: plan.volumes.clone(),
        containers: plan
            .containers
            .iter()
            .enumerate()
            .map(|(index, container)| PlannedMounts {
                index,
                mounts: container.volume_mounts.clone(),
            })
            .collect(),
    };
    write_json(&pod.join("mount-plan.json"), &planned, 0o644)
}

/// ConfigMap and Secret directories are replaced. EmptyDir is created once
/// and kept. Slab volumes are not files on the pod directory.
fn materialize_volumes(pod: &Path, plan: &PodPlan) -> ApiResult<()> {
    for (index, vol) in plan.volumes.iter().enumerate() {
        match &vol.source {
            VolumeSrc::EmptyDir => {
                let dir = volume_host_dir(pod, index, &vol.name);
                std::fs::create_dir_all(&dir).map_err(|e| volume_io(&dir, e))?;
            }
            VolumeSrc::ConfigMap(name) => {
                let dir = volume_host_dir(pod, index, &vol.name);
                match plan.config_maps.get(name) {
                    Some(files) => write_projected(&dir, files, 0o644, 0o755)?,
                    None => reset_dir(&dir)?,
                }
            }
            VolumeSrc::Secret(name) => {
                let dir = volume_host_dir(pod, index, &vol.name);
                match plan.secrets.get(name) {
                    Some(files) => write_projected(&dir, files, 0o400, 0o700)?,
                    None => {
                        reset_dir(&dir)?;
                        set_mode(&dir, 0o700)?;
                    }
                }
            }
            VolumeSrc::Volume(_) => {}
        }
    }
    Ok(())
}

fn write_projected(dir: &Path, files: &Projected, file_mode: u32, dir_mode: u32) -> ApiResult<()> {
    reset_dir(dir)?;
    set_mode(dir, dir_mode)?;
    let mut keys: Vec<&String> = files.data.keys().collect();
    for key in files.binary_data.keys() {
        if !files.data.contains_key(key) {
            keys.push(key);
        }
    }
    keys.sort();
    for key in keys {
        let Some(rel) = rel_under(key) else {
            continue;
        };
        let Some(bytes) = projected_bytes(files, key) else {
            continue;
        };
        write_mode(&dir.join(rel), &bytes, file_mode)?;
    }
    Ok(())
}

fn projected_bytes(files: &Projected, key: &str) -> Option<Vec<u8>> {
    if let Some(text) = files.data.get(key) {
        return Some(text.as_bytes().to_vec());
    }
    decode_b64(files.binary_data.get(key)?)
}

fn prepared_volume(
    pod: &Path,
    index: usize,
    vol: &SandboxVolume,
    disks: &[AttachedDisk],
) -> Option<PreparedVolume> {
    match &vol.source {
        VolumeSrc::EmptyDir | VolumeSrc::ConfigMap(_) | VolumeSrc::Secret(_) => {
            Some(PreparedVolume {
                host_path: volume_host_dir(pod, index, &vol.name),
                directory: true,
                force_read_only: false,
            })
        }
        VolumeSrc::Volume(id) => {
            let disk = disks.iter().find(|d| &d.volume == id)?;
            let host_path = PathBuf::from(&disk.path);
            let directory = std::fs::metadata(&host_path)
                .map(|m| m.is_dir())
                .unwrap_or(false);
            Some(PreparedVolume {
                host_path,
                directory,
                force_read_only: disk.read_only,
            })
        }
    }
}

fn mount_source(prepared: &PreparedVolume, sub_path: &str) -> Option<(String, bool)> {
    let (source, directory) = if sub_path.is_empty() {
        (prepared.host_path.clone(), prepared.directory)
    } else {
        let rel = rel_under(sub_path)?;
        let source = prepared.host_path.join(rel);
        let directory = std::fs::metadata(&source)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        (source, directory)
    };
    Some((source.to_string_lossy().into_owned(), directory))
}

fn oci_binds(
    pod: &Path,
    volumes: &[SandboxVolume],
    mounts: &[SandboxMount],
    disks: &[AttachedDisk],
) -> Vec<OciBind> {
    let mut out = Vec::new();
    for mount in mounts {
        if mount.name.is_empty() || !valid_destination(&mount.mount_path) {
            continue;
        }
        let Some(index) = volumes.iter().position(|v| v.name == mount.name) else {
            continue;
        };
        let Some(prepared) = prepared_volume(pod, index, &volumes[index], disks) else {
            continue;
        };
        let Some((source, directory)) = mount_source(&prepared, &mount.sub_path) else {
            continue;
        };
        out.push(OciBind {
            destination: mount.mount_path.clone(),
            source,
            read_only: prepared.force_read_only || mount.read_only,
            directory,
        });
    }
    out
}

fn host_path_ok(path: &str) -> ApiResult<()> {
    let p = Path::new(path);
    if path.is_empty() || path.contains('\0') || !p.is_absolute() || has_parent_dir(p) {
        return Err(ApiError::bad_request(
            "volume path must be an absolute path",
        ));
    }
    Ok(())
}

fn check_device_name(name: &str) -> ApiResult<()> {
    let b = name.as_bytes();
    if b.len() == 3 && b[0] == b'v' && b[1] == b'd' && (b'b'..=b'z').contains(&b[2]) {
        return Ok(());
    }
    Err(ApiError::bad_request(
        "device must be a virtio name from vdb to vdz",
    ))
}

fn next_device(disks: &[AttachedDisk]) -> ApiResult<String> {
    for c in b'b'..=b'z' {
        let name = format!("vd{}", c as char);
        if disks.iter().all(|d| d.device != name) {
            return Ok(name);
        }
    }
    Err(ApiError::unprocessable("no free virtio device name"))
}

fn disks_path(bundle: &Path) -> PathBuf {
    bundle.join("disks.json")
}

fn read_disks(bundle: &Path) -> ApiResult<Vec<AttachedDisk>> {
    let path = disks_path(bundle);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            ApiError::unavailable(format!("youki runtime: reading {}: {e}", path.display()))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(read_io(&path, e)),
    }
}

fn write_disks(bundle: &Path, disks: &[AttachedDisk]) -> ApiResult<()> {
    write_json(&disks_path(bundle), &disks, 0o644)
}

fn refresh_disk_mounts(bundle: &Path) -> ApiResult<()> {
    let path = bundle.join("mount-plan.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(read_io(&path, e)),
    };
    let plan: MountPlan = serde_json::from_slice(&bytes).map_err(|e| {
        ApiError::unavailable(format!("youki runtime: reading {}: {e}", path.display()))
    })?;
    let disks = read_disks(bundle)?;
    for container in &plan.containers {
        let binds = oci_binds(bundle, &plan.volumes, &container.mounts, &disks);
        let config = container_dir(bundle, container.index).join("config.json");
        patch_mounts(&config, &binds)?;
    }
    Ok(())
}

fn patch_mounts(config: &Path, binds: &[OciBind]) -> ApiResult<()> {
    let bytes = match std::fs::read(config) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(read_io(config, e)),
    };
    let mut spec: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        ApiError::unavailable(format!("youki runtime: reading {}: {e}", config.display()))
    })?;
    let mut mounts = Vec::new();
    if let Some(existing) = spec.get("mounts").and_then(|m| m.as_array()) {
        for mount in existing {
            let dest = mount
                .get("destination")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            if dest == "/proc" || dest == "/dev" || dest == "/sys" {
                mounts.push(mount.clone());
            }
        }
    }
    for bind in binds {
        mounts.push(bind_mount(bind));
    }
    let obj = spec.as_object_mut().ok_or_else(|| {
        ApiError::unavailable(format!(
            "youki runtime: reading {}: config is not an object",
            config.display()
        ))
    })?;
    obj.insert("mounts".into(), serde_json::Value::Array(mounts));
    write_json(config, &spec, 0o600)
}

fn write_json(path: &Path, value: &impl Serialize, mode: u32) -> ApiResult<()> {
    let text = serde_json::to_vec_pretty(value).map_err(|e| {
        ApiError::internal(format!("youki runtime: encoding {}: {e}", path.display()))
    })?;
    write_mode(path, &text, mode)
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) -> ApiResult<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| volume_io(parent, e))?;
        }
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        let ft = meta.file_type();
        let removed = if ft.is_symlink() || !ft.is_dir() {
            std::fs::remove_file(path)
        } else {
            std::fs::remove_dir_all(path)
        };
        removed.map_err(|e| volume_io(path, e))?;
    }
    std::fs::write(path, bytes).map_err(|e| volume_io(path, e))?;
    set_mode(path, mode)
}

fn set_mode(path: &Path, mode: u32) -> ApiResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .map_err(|e| volume_io(path, e))?
            .permissions();
        perms.set_mode(mode);
        std::fs::set_permissions(path, perms).map_err(|e| volume_io(path, e))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

fn volume_io(path: &Path, err: std::io::Error) -> ApiError {
    ApiError::unavailable(format!("youki runtime: writing {}: {err}", path.display()))
}

fn read_io(path: &Path, err: std::io::Error) -> ApiError {
    ApiError::unavailable(format!("youki runtime: reading {}: {err}", path.display()))
}

fn parse_state_pid(stdout: &[u8]) -> Option<u32> {
    let value: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    let pid = value.get("pid")?.as_u64()?;
    u32::try_from(pid).ok().filter(|pid| *pid > 0)
}

/// Create the sandbox, then every container, joining the sandbox pid, network,
/// ipc and uts namespaces. `state.json` is written only after they are running.
async fn start_pod(
    bin: &Path,
    mode: &RunMode,
    bundle: &Path,
    req: &MachineRequest,
    plan: &PodPlan,
    prior: bool,
) -> ApiResult<RunningMachine> {
    if prior {
        if let Some(saved) = read_state(bundle).await? {
            for cid in saved.containers.iter().rev() {
                if let Err(e) = exec_youki(bin, mode, &["delete", cid], None).await {
                    if !container_missing(&e) {
                        return Err(e);
                    }
                }
            }
        }
    }
    let limits = Limits {
        vcpus: req.resources.vcpus,
        memory_mib: req.resources.memory_mib,
    };
    let hostname = if req.name.is_empty() {
        req.id.to_string()
    } else {
        req.name.clone()
    };
    let id = req.id.clone();
    let addresses = nic_addresses(req);
    let bundle_prep = bundle.to_path_buf();
    let plan_prep = plan.clone();
    let host_prep = hostname;
    let image = req.image.clone();
    let mode_prep = mode.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_pod(
            &bundle_prep,
            &host_prep,
            &limits,
            &plan_prep,
            &image,
            &mode_prep,
        )
    })
    .await
    .map_err(|_| ApiError::unavailable("youki runtime: image unpack stopped"))?;
    let processes = match prepared {
        Ok(processes) => processes,
        Err(e) => {
            if !prior {
                let _ = remove_bundle(bundle).await;
            }
            return Err(e);
        }
    };
    let id_str = id.to_string();
    let bundle_str = path_arg(bundle);
    if !prior {
        if let Err(e) = exec_youki(
            bin,
            mode,
            &["create", "--bundle", &bundle_str, &id_str],
            None,
        )
        .await
        {
            let _ = remove_bundle(bundle).await;
            return Err(e);
        }
        if let Err(e) = exec_youki(bin, mode, &["start", &id_str], None).await {
            let _ = exec_youki(bin, mode, &["delete", &id_str], None).await;
            let _ = remove_bundle(bundle).await;
            return Err(e);
        }
    } else {
        exec_youki(bin, mode, &["start", &id_str], None).await?;
    }
    let state = match run_youki(bin, mode, &["state", &id_str], None).await {
        Ok(output) => output,
        Err(e) => {
            if !prior {
                abort_new_pod(bin, mode, bundle, &id_str, &[]).await;
            }
            return Err(e);
        }
    };
    let Some(pid) = parse_state_pid(&state.stdout) else {
        if !prior {
            abort_new_pod(bin, mode, bundle, &id_str, &[]).await;
        }
        return Err(ApiError::unavailable(
            "youki runtime: sandbox state has no pid",
        ));
    };
    let bundle_cfg = bundle.to_path_buf();
    let plan_cfg = plan.clone();
    let limits_cfg = limits;
    let mode_cfg = mode.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || {
        let disks = read_disks(&bundle_cfg)?;
        write_container_configs(
            &bundle_cfg,
            &plan_cfg,
            &processes,
            &disks,
            &limits_cfg,
            pid,
            &mode_cfg,
        )
    })
    .await
    .map_err(|_| ApiError::unavailable("youki runtime: image unpack stopped"))?
    {
        if !prior {
            abort_new_pod(bin, mode, bundle, &id_str, &[]).await;
        }
        return Err(e);
    }
    let user_ns = user_ns_join(mode, pid);
    let mut started = Vec::new();
    for index in 0..plan.containers.len() {
        let cid = container_youki_id(&id, index);
        let cdir = path_arg(&container_dir(bundle, index));
        if let Err(e) = exec_youki(
            bin,
            mode,
            &["create", "--bundle", &cdir, &cid],
            user_ns.as_deref(),
        )
        .await
        {
            if !prior {
                abort_new_pod(bin, mode, bundle, &id_str, &started).await;
            }
            return Err(e);
        }
        if let Err(e) = exec_youki(bin, mode, &["start", &cid], user_ns.as_deref()).await {
            if !prior {
                let _ = exec_youki(bin, mode, &["delete", &cid], None).await;
                abort_new_pod(bin, mode, bundle, &id_str, &started).await;
            }
            return Err(e);
        }
        started.push(cid);
    }
    write_state(
        bundle,
        &Saved {
            id: id_str,
            addresses: addresses.clone(),
            power: PowerState::On,
            containers: started,
            container_names: plan.containers.iter().map(|c| c.name.clone()).collect(),
            probes: crate::probe::probes_in_user_data(req.user_data.as_deref()),
        },
    )
    .await?;
    Ok(RunningMachine {
        id,
        addresses,
        power: PowerState::On,
    })
}

async fn abort_new_pod(
    bin: &Path,
    mode: &RunMode,
    bundle: &Path,
    sandbox_id: &str,
    created: &[String],
) {
    for cid in created.iter().rev() {
        let _ = exec_youki(bin, mode, &["delete", cid], None).await;
    }
    let _ = exec_youki(bin, mode, &["delete", sandbox_id], None).await;
    let _ = remove_bundle(bundle).await;
}

fn validate_exec(argv: &[String], env: &[(String, String)], cwd: Option<&str>) -> ApiResult<()> {
    if argv.is_empty() {
        return Err(ApiError::bad_request(
            "youki runtime: exec command is empty",
        ));
    }
    if argv.iter().any(|arg| arg.contains('\0')) {
        return Err(ApiError::bad_request(
            "youki runtime: exec command contains a NUL byte",
        ));
    }
    for (name, value) in env {
        if !valid_env_name(name) || value.contains('\0') || value.contains('\n') {
            return Err(ApiError::bad_request(
                "youki runtime: exec env must be a NAME=value pair",
            ));
        }
    }
    if let Some(dir) = cwd {
        if dir.is_empty() || dir.starts_with('-') || dir.contains('\0') {
            return Err(ApiError::bad_request(
                "youki runtime: exec working directory is not a path",
            ));
        }
    }
    Ok(())
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn resolve_container<'a>(saved: &'a Saved, name: &str) -> ApiResult<&'a str> {
    if name.is_empty() {
        return Err(ApiError::bad_request(
            "youki runtime: container name is empty",
        ));
    }
    if let Some(index) = saved.container_names.iter().position(|n| n == name) {
        if let Some(id) = saved.containers.get(index) {
            return Ok(id);
        }
    }
    if let Some(id) = saved.containers.iter().find(|id| id.as_str() == name) {
        return Ok(id);
    }
    Err(ApiError::not_found(format!(
        "youki runtime: container {name} is not in this pod"
    )))
}

fn exec_argv(
    container_id: &str,
    argv: &[String],
    env: &[(String, String)],
    cwd: Option<&str>,
) -> Vec<String> {
    let mut args = Vec::with_capacity(4 + env.len() * 2 + argv.len());
    args.push("exec".into());
    if let Some(dir) = cwd {
        args.push("--cwd".into());
        args.push(dir.to_owned());
    }
    for (name, value) in env {
        args.push("--env".into());
        args.push(format!("{name}={value}"));
    }
    args.push(container_id.to_owned());
    args.push("--".into());
    args.extend(argv.iter().cloned());
    args
}

/// Kills the youki process group if a wait is abandoned. Disarm after a
/// successful wait so a recycled pid is left alone.
struct GroupKill {
    pid: Option<u32>,
}

impl GroupKill {
    fn arm(pid: Option<u32>) -> Self {
        Self { pid }
    }

    fn disarm(&mut self) {
        self.pid = None;
    }
}

impl Drop for GroupKill {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
            signal_group(pid);
        }
    }
}

fn signal_group(pid: u32) {
    #[cfg(unix)]
    {
        let Ok(pid) = i32::try_from(pid) else {
            return;
        };
        if pid <= 0 {
            return;
        }
        // SAFETY: `pid` is a positive process-group id created with
        // `process_group(0)`. A negative pid asks the kernel to signal that
        // group. A stale id fails the call; the result is ignored.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

fn timeout_label(timeout: Duration) -> String {
    let secs = timeout.as_secs();
    let millis = timeout.subsec_millis();
    if millis == 0 {
        format!("{secs}s")
    } else if secs == 0 {
        format!("{millis}ms")
    } else {
        format!("{secs}.{millis:03}s")
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_exec(
    bin: &Path,
    privilege: &Privilege,
    container_id: &str,
    argv: &[String],
    env: &[(String, String)],
    cwd: Option<&str>,
    timeout: Duration,
    output_cap: usize,
) -> ApiResult<ExecOutput> {
    let args = exec_argv(container_id, argv, env, cwd);
    let mut cmd = youki_command(bin, privilege);
    cmd.args(&args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    cmd.process_group(0);
    let program = match privilege {
        Privilege::Sudo { sudo } => sudo.display().to_string(),
        Privilege::Root | Privilege::Rootless { .. } => bin.display().to_string(),
    };
    let mut child = cmd
        .spawn()
        .map_err(|e| ApiError::unavailable(format!("youki runtime: cannot run {program}: {e}")))?;
    let mut group = GroupKill::arm(child.id());
    let stdout = spawn_reader(child.stdout.take(), output_cap);
    let stderr = spawn_reader(child.stderr.take(), output_cap);
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            group.disarm();
            status
        }
        Ok(Err(e)) => {
            drop(group);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            let _ = drain_text(stdout).await;
            let _ = drain_text(stderr).await;
            return Err(ApiError::unavailable(format!(
                "youki runtime: {}: {e}",
                bin.display()
            )));
        }
        Err(_) => {
            drop(group);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            let _ = drain_text(stdout).await;
            let _ = drain_text(stderr).await;
            return Err(ApiError::unavailable(format!(
                "youki runtime: exec timed out after {}",
                timeout_label(timeout)
            )));
        }
    };
    Ok(ExecOutput {
        exit_code: status.code().unwrap_or(1),
        stdout: drain_text(stdout).await,
        stderr: drain_text(stderr).await,
    })
}

fn spawn_reader<R>(pipe: Option<R>, cap: usize) -> tokio::task::JoinHandle<String>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        match pipe {
            Some(pipe) => bytes_to_text(&read_capped(pipe, cap).await),
            None => String::new(),
        }
    })
}

async fn read_capped<R>(mut pipe: R, cap: usize) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = cap.saturating_sub(buf.len());
                if room == 0 {
                    continue;
                }
                let take = n.min(room);
                buf.extend_from_slice(&chunk[..take]);
            }
        }
    }
    buf
}

async fn drain_text(task: tokio::task::JoinHandle<String>) -> String {
    match tokio::time::timeout(Duration::from_secs(2), task).await {
        Ok(Ok(text)) => text,
        _ => String::new(),
    }
}

/// Keep a trailing partial UTF-8 sequence out of the lossy conversion so a
/// cap that lands mid-character does not invent a replacement character.
fn bytes_to_text(buf: &[u8]) -> String {
    let end = utf8_prefix_end(buf);
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

fn utf8_prefix_end(buf: &[u8]) -> usize {
    if std::str::from_utf8(buf).is_ok() {
        return buf.len();
    }
    let mut end = buf.len();
    while end > 0 && (buf[end - 1] & 0b1100_0000) == 0b1000_0000 {
        end -= 1;
    }
    if end > 0 {
        let lead = buf[end - 1];
        let width = match lead {
            0x00..=0x7F => 1,
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => 0,
        };
        let have = buf.len() - (end - 1);
        if width == 0 || have < width {
            end -= 1;
        }
    }
    end
}

/// Rootless container create/start must already be inside the sandbox user
/// namespace. youki 0.7 treats a joined user namespace (a path, no new map)
/// as "no user namespace" when the calling process is outside one.
fn user_ns_join(mode: &RunMode, pid: u32) -> Option<PathBuf> {
    match &mode.privilege {
        Privilege::Rootless { .. } => Some(PathBuf::from(format!("/proc/{pid}/ns/user"))),
        Privilege::Root | Privilege::Sudo { .. } => None,
    }
}

async fn exec_youki(
    bin: &Path,
    mode: &RunMode,
    args: &[&str],
    join_user_ns: Option<&Path>,
) -> ApiResult<()> {
    run_youki(bin, mode, args, join_user_ns).await.map(|_| ())
}

async fn run_youki(
    bin: &Path,
    mode: &RunMode,
    args: &[&str],
    join_user_ns: Option<&Path>,
) -> ApiResult<std::process::Output> {
    let mut cmd = youki_command(bin, &mode.privilege);
    cmd.args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    arm_user_ns_join(&mut cmd, join_user_ns);
    let program = match &mode.privilege {
        Privilege::Sudo { sudo } => sudo.display().to_string(),
        Privilege::Root | Privilege::Rootless { .. } => bin.display().to_string(),
    };
    let child = cmd
        .spawn()
        .map_err(|e| ApiError::unavailable(format!("youki runtime: cannot run {program}: {e}")))?;
    let output = match tokio::time::timeout(TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return Err(ApiError::unavailable(format!(
                "youki runtime: {}: {e}",
                bin.display()
            )))
        }
        Err(_) => {
            return Err(ApiError::unavailable(format!(
                "youki runtime: {} timed out after 30s",
                args.first().copied().unwrap_or("youki")
            )))
        }
    };
    if output.status.success() {
        return Ok(output);
    }
    let op = args.first().copied().unwrap_or("youki");
    Err(ApiError::unavailable(format!(
        "youki runtime: {op} failed: {}",
        stderr_tail(&output.stderr, output.status)
    )))
}

fn youki_command(bin: &Path, privilege: &Privilege) -> Command {
    match privilege {
        Privilege::Sudo { sudo } => {
            let mut cmd = Command::new(sudo);
            cmd.arg("-n").arg(bin);
            cmd
        }
        Privilege::Root | Privilege::Rootless { .. } => Command::new(bin),
    }
}

fn arm_user_ns_join(cmd: &mut Command, path: Option<&Path>) {
    #[cfg(target_os = "linux")]
    {
        let Some(path) = path else {
            return;
        };
        // A missing path is the test double (the sandbox pid is not real).
        // A path that exists and cannot be joined fails the command.
        if !path.exists() {
            return;
        }
        let path = path.to_path_buf();
        // SAFETY: the closure runs in the forked child before exec. That child
        // is single-threaded, so setns(CLONE_NEWUSER) is valid there.
        unsafe {
            std::os::unix::process::CommandExt::pre_exec(cmd.as_std_mut(), move || {
                enter_user_ns(&path)
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (cmd, path);
    }
}

#[cfg(target_os = "linux")]
fn enter_user_ns(path: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "youki runtime: user namespace path",
        )
    })?;
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let rc = unsafe { libc::setns(fd, libc::CLONE_NEWUSER) };
    let err = std::io::Error::last_os_error();
    unsafe { libc::close(fd) };
    if rc == 0 {
        Ok(())
    } else {
        Err(err)
    }
}

fn stderr_tail(stderr: &[u8], status: std::process::ExitStatus) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(20);
    let mut tail = lines[start..].join("\n");
    const MAX: usize = 1500;
    if tail.chars().count() > MAX {
        let skip = tail.chars().count() - MAX;
        tail = tail.chars().skip(skip).collect();
    }
    if tail.is_empty() {
        format!("exit {status}")
    } else {
        tail
    }
}

fn container_missing(err: &ApiError) -> bool {
    let m = err.message.to_ascii_lowercase();
    m.contains("not found")
        || m.contains("does not exist")
        || m.contains("doesn't exist")
        || m.contains("no such container")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MachineKind;
    use pistack_types::{error::ErrorCode, Resources};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        super::youki_env_lock()
    }

    struct EnvGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        /// Points youki at the test binary and forces the root OCI config.
        /// Sudo, netns, and user-namespace overrides from the developer
        /// environment are cleared for the duration of the test.
        fn set(bin: &Path, state: &Path) -> Self {
            let keys = [
                "PISTACK_YOUKI_BIN",
                "PISTACK_YOUKI_STATE",
                SUDO_ENV,
                SUDO_BIN_ENV,
                NETNS_ENV,
                "PISTACK_YOUKI_TEST_EUID",
                "PISTACK_YOUKI_TEST_EGID",
                "PISTACK_YOUKI_TEST_USERNS",
            ];
            let saved = keys
                .into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect();
            std::env::set_var("PISTACK_YOUKI_BIN", bin);
            std::env::set_var("PISTACK_YOUKI_STATE", state);
            std::env::set_var("PISTACK_YOUKI_TEST_EUID", "0");
            for key in [
                SUDO_ENV,
                SUDO_BIN_ENV,
                NETNS_ENV,
                "PISTACK_YOUKI_TEST_EGID",
                "PISTACK_YOUKI_TEST_USERNS",
            ] {
                std::env::remove_var(key);
            }
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, prev) in self.saved.drain(..) {
                restore(key, prev);
            }
        }
    }

    fn restore(key: &str, prev: Option<std::ffi::OsString>) {
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    struct EnvSlot {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvSlot {
        fn clear(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, prev }
        }
    }

    impl Drop for EnvSlot {
        fn drop(&mut self) {
            restore(self.key, self.prev.take());
        }
    }

    fn write_script(path: &Path, log: &Path, exit_code: i32) {
        let quoted = format!("'{}'", log.display().to_string().replace('\'', "'\\''"));
        let stderr = if exit_code == 0 {
            String::new()
        } else {
            "echo 'youki exploded' >&2\n".to_owned()
        };
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {quoted}\nprintf '\\n' >> {quoted}\nif [ \"$1\" = state ]; then\n  printf '%s\\n' '{{\"pid\":4242,\"status\":\"running\"}}'\nfi\n{stderr}exit {exit_code}\n"
        );
        std::fs::write(path, body).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    fn req(name: &str, image: &str, address: &str) -> MachineRequest {
        MachineRequest {
            id: Id::new(kind::MACHINE),
            name: name.into(),
            kind: MachineKind::Container,
            image: image.into(),
            resources: Resources {
                vcpus: 1,
                memory_mib: 128,
                disk_gib: 1,
                gpus: 0,
            },
            networks: vec![],
            nics: vec![crate::runtime::NicAttachment {
                address: address.into(),
                ..Default::default()
            }],
            volumes: vec![],
            user_data: Some("do-not-log-user-data".into()),
            ssh_keys: vec!["do-not-log-ssh-key".into()],
            overlay: None,
            bmc_address: None,
            bmc_user: None,
        }
    }

    fn arg_lines(log: &str) -> Vec<String> {
        log.lines()
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn count_arg(lines: &[String], arg: &str) -> usize {
        lines.iter().filter(|l| l.as_str() == arg).count()
    }

    fn sandbox_doc(image: &str) -> String {
        serde_json::json!({
            "kind": "PodSandbox",
            "containers": [
                {
                    "name": "web",
                    "image": image,
                    "command": ["/usr/bin/web"],
                    "args": ["--serve"],
                    "working_dir": "/work",
                    "env": [
                        {"name": "APP_MODE", "value": "prod"},
                        {"name": "SECRET", "value_from": {"secret": "do-not-log-user-data"}}
                    ]
                },
                {
                    "name": "side",
                    "image": image,
                    "command": ["/usr/bin/side"],
                    "args": ["--watch"]
                }
            ]
        })
        .to_string()
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn ns_path(cfg: &serde_json::Value, kind: &str) -> Option<String> {
        cfg["linux"]["namespaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["type"] == kind)
            .and_then(|n| n.get("path"))
            .and_then(|p| p.as_str())
            .map(str::to_owned)
    }

    fn tree_text(root: &Path) -> String {
        let mut out = String::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            for ent in rd.flatten() {
                let path = ent.path();
                let Ok(meta) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                if meta.file_type().is_symlink() {
                    if let Ok(target) = std::fs::read_link(&path) {
                        out.push_str(&target.to_string_lossy());
                    }
                } else if meta.is_dir() {
                    stack.push(path);
                } else if meta.is_file() {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        out.push_str(&text);
                    }
                }
            }
        }
        out
    }

    fn put_octal(dst: &mut [u8], val: u64) {
        let text = format!("{val:o}");
        assert!(text.len() < dst.len());
        dst[..text.len()].copy_from_slice(text.as_bytes());
        dst[text.len()] = 0;
    }

    fn tar_header(name: &str, size: u64, flag: u8) -> [u8; 512] {
        let mut header = [0u8; 512];
        assert!(name.len() < 100, "{name}");
        header[..name.len()].copy_from_slice(name.as_bytes());
        put_octal(&mut header[100..108], 0o644);
        put_octal(&mut header[108..116], 0);
        put_octal(&mut header[116..124], 0);
        put_octal(&mut header[124..136], size);
        put_octal(&mut header[136..148], 0);
        header[148..156].fill(b' ');
        header[156] = flag;
        header[257..262].copy_from_slice(b"ustar");
        header[263] = b'0';
        header[264] = b'0';
        let sum: u64 = header.iter().map(|b| u64::from(*b)).sum();
        let text = format!("{sum:06o}");
        header[148..154].copy_from_slice(text.as_bytes());
        header[154] = 0;
        header[155] = b' ';
        header
    }

    fn tar_file(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        for (name, data) in entries {
            buf.extend_from_slice(&tar_header(name, data.len() as u64, b'0'));
            buf.extend_from_slice(data);
            let pad = (512 - (data.len() % 512)) % 512;
            buf.extend(std::iter::repeat_n(0u8, pad));
        }
        buf.extend(std::iter::repeat_n(0u8, 1024));
        buf
    }

    fn gzip_bytes(raw: &[u8]) -> Vec<u8> {
        let mut child = std::process::Command::new("gzip")
            .arg("-c")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("gzip");
        child.stdin.take().unwrap().write_all(raw).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "gzip");
        out.stdout
    }

    fn docker_save() -> Vec<u8> {
        let layer1 = tar_file(&[("keep", b"yes"), ("gone", b"old"), ("sub/old", b"x")]);
        let layer2 = gzip_bytes(&tar_file(&[
            (".wh.gone", b""),
            ("sub/.wh..wh..opq", b""),
            ("sub/new", b"n"),
        ]));
        let config = br#"{"config":{"Entrypoint":["/bin/app"],"Cmd":["serve"],"Env":["APP_MODE=image"],"WorkingDir":"/app"}}"#;
        let manifest = br#"[{"Config":"config.json","Layers":["layer1.tar","layer2.tar.gz"]}]"#;
        tar_file(&[
            ("manifest.json", manifest),
            ("config.json", config),
            ("layer1.tar", &layer1),
            ("layer2.tar.gz", &layer2),
        ])
    }

    #[test]
    fn sandbox_command_replaces_the_image_process() {
        let cfg = ImageConfig {
            entrypoint: vec!["/entry".into()],
            cmd: vec!["from-image".into()],
            env: vec!["FROM=image".into()],
            working_dir: "/image".into(),
        };
        let both = Launch {
            command: vec!["/cmd".into()],
            args: vec!["--a".into()],
            env: vec![("APP_MODE".into(), "prod".into())],
            cwd: "/work".into(),
            ..Launch::default()
        };
        let (args, env, cwd) = process_spec(Some(&both), &cfg);
        assert_eq!(args, vec!["/cmd".to_string(), "--a".into()]);
        assert_eq!(cwd, "/work");
        assert!(env.iter().any(|e| e == "APP_MODE=prod"));
        assert!(env.iter().any(|e| e == "FROM=image"));
        let cmd_only = Launch {
            command: vec!["/cmd".into()],
            ..Launch::default()
        };
        assert_eq!(
            process_spec(Some(&cmd_only), &cfg).0,
            vec!["/cmd".to_string()]
        );
        let args_only = Launch {
            args: vec!["--only".into()],
            ..Launch::default()
        };
        assert_eq!(
            process_spec(Some(&args_only), &cfg).0,
            vec!["/entry".to_string(), "--only".into()]
        );
        assert_eq!(
            process_spec(None, &cfg).0,
            vec!["/entry".to_string(), "from-image".into()]
        );
        assert_eq!(
            process_spec(None, &ImageConfig::default()).0,
            vec!["/bin/sh".to_string()]
        );
        assert_eq!(process_spec(None, &cfg).2, "/image");

        let mut request = req("web", "ignored", "10.0.0.1");
        request.user_data = Some(sandbox_doc("/tmp/image"));
        let launch = launch_from(&request).unwrap();
        assert_eq!(launch.command, vec!["/usr/bin/web".to_string()]);
        assert_eq!(launch.args, vec!["--serve".to_string()]);
        assert_eq!(launch.env, vec![("APP_MODE".into(), "prod".into())]);
        assert!(launch
            .env
            .iter()
            .all(|(k, v)| !k.contains("do-not-log") && !v.contains("do-not-log")));
        request.user_data = Some("do-not-log-user-data".into());
        assert!(launch_from(&request).is_none());
        request.user_data = Some(r#"{"kind":"Other"}"#.into());
        assert!(launch_from(&request).is_none());
        request.user_data = Some(r#"{"kind":"PodSandbox","containers":"no"}"#.into());
        let err = sandbox_plan(&request).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("sandbox document is invalid"), "{err}");
        assert!(!err.message.contains("do-not-log"), "{err}");
    }

    #[tokio::test]
    async fn start_is_idempotent_stop_kills_and_missing_bin_is_unavailable() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let log = tmp.join("youki.log");
        let bin = tmp.join("youki");
        let state = tmp.join("state");
        write_script(&bin, &log, 0);
        let _env = EnvGuard::set(&bin, &state);

        let rt = YoukiRuntime;
        let unknown = Id::new(kind::MACHINE);
        assert_eq!(
            rt.stop(&unknown).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        rt.delete(&unknown).await.unwrap();

        let external = tmp.join("external-rootfs");
        std::fs::create_dir_all(external.join("bin")).unwrap();
        std::fs::write(external.join("marker"), "keep").unwrap();
        std::fs::write(external.join("bin").join("web"), "#!/bin/sh\n").unwrap();
        let image = external.to_str().unwrap();
        let mut request = req("web", "not-a-path", "10.1.0.5");
        request.user_data = Some(sandbox_doc(image));
        let started = rt.start(&request).await.unwrap();
        assert_eq!(started.id, request.id);
        assert_eq!(started.power, PowerState::On);
        assert_eq!(started.addresses, vec!["10.1.0.5".to_string()]);

        let bundle = state.join(request.id.to_string());
        let cfg: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bundle.join("config.json")).unwrap()).unwrap();
        assert_eq!(cfg["root"]["path"], "rootfs");
        assert_eq!(cfg["root"]["readonly"], false);
        assert_eq!(cfg["hostname"], "web");
        assert_eq!(cfg["process"]["args"], serde_json::json!(["/pause"]));
        assert_eq!(cfg["process"]["cwd"], "/");
        let sandbox_ns = cfg["linux"]["namespaces"].as_array().unwrap();
        assert!(sandbox_ns.iter().all(|n| n.get("path").is_none()));
        assert_eq!(mode_of(&bundle.join("rootfs").join("pause")), 0o755);
        assert!(!bundle.join("image").exists());
        let rootfs_meta = std::fs::symlink_metadata(bundle.join("rootfs")).unwrap();
        assert!(rootfs_meta.file_type().is_dir());
        assert!(!rootfs_meta.file_type().is_symlink());
        assert!(!bundle.join("rootfs").join("marker").exists());

        let c0 = bundle.join("containers").join("0");
        let c1 = bundle.join("containers").join("1");
        let web: serde_json::Value =
            serde_json::from_slice(&std::fs::read(c0.join("config.json")).unwrap()).unwrap();
        let side: serde_json::Value =
            serde_json::from_slice(&std::fs::read(c1.join("config.json")).unwrap()).unwrap();
        assert_eq!(
            web["process"]["args"],
            serde_json::json!(["/usr/bin/web", "--serve"])
        );
        assert_eq!(web["process"]["cwd"], "/work");
        assert!(web.get("hostname").is_none());
        let env = web["process"]["env"].as_array().unwrap();
        assert!(env.iter().any(|v| v == "APP_MODE=prod"));
        assert!(env.iter().all(|v| {
            let text = v.as_str().unwrap_or("");
            !text.contains("do-not-log")
        }));
        assert_eq!(
            side["process"]["args"],
            serde_json::json!(["/usr/bin/side", "--watch"])
        );
        assert!(side.get("hostname").is_none());
        for cfg in [&web, &side] {
            assert_eq!(
                ns_path(cfg, "network").as_deref(),
                Some("/proc/4242/ns/net")
            );
            assert_eq!(ns_path(cfg, "pid").as_deref(), Some("/proc/4242/ns/pid"));
            assert_eq!(ns_path(cfg, "ipc").as_deref(), Some("/proc/4242/ns/ipc"));
            assert_eq!(ns_path(cfg, "uts").as_deref(), Some("/proc/4242/ns/uts"));
            assert_eq!(ns_path(cfg, "mount"), None);
        }
        assert_eq!(mode_of(&c0.join("config.json")), 0o600);
        assert_eq!(
            std::fs::read_to_string(c0.join("rootfs").join("marker")).unwrap(),
            "keep"
        );
        assert_eq!(
            std::fs::read_to_string(c1.join("rootfs").join("marker")).unwrap(),
            "keep"
        );
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bundle.join("state.json")).unwrap()).unwrap();
        assert_eq!(
            saved["containers"],
            serde_json::json!([format!("{}-0", request.id), format!("{}-1", request.id)])
        );
        let bundle_text = tree_text(&bundle);
        assert!(!bundle_text.contains("do-not-log-user-data"));
        assert!(!bundle_text.contains("do-not-log-ssh-key"));

        let after_first = std::fs::read_to_string(&log).unwrap();
        assert!(!after_first.contains("do-not-log-user-data"));
        assert!(!after_first.contains("do-not-log-ssh-key"));
        let lines = arg_lines(&after_first);
        assert_eq!(count_arg(&lines, "create"), 3);
        assert_eq!(count_arg(&lines, "start"), 3);
        assert_eq!(count_arg(&lines, "state"), 1);
        assert_eq!(count_arg(&lines, "kill"), 0);
        let bundle_arg = bundle.to_string_lossy().into_owned();
        let id_arg = request.id.to_string();
        assert!(lines.windows(4).any(|w| {
            w[0] == "create" && w[1] == "--bundle" && w[2] == bundle_arg && w[3] == id_arg
        }));
        let c0_arg = c0.to_string_lossy().into_owned();
        assert!(lines.windows(4).any(|w| {
            w[0] == "create"
                && w[1] == "--bundle"
                && w[2] == c0_arg
                && w[3] == format!("{}-0", request.id)
        }));

        let again = rt.start(&request).await.unwrap();
        assert_eq!(again.power, PowerState::On);
        assert_eq!(again.addresses, vec!["10.1.0.5".to_string()]);
        let after_second = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            after_first, after_second,
            "second start must not invoke youki"
        );

        let listed = rt.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, request.id);
        assert_eq!(listed[0].power, PowerState::On);

        rt.stop(&request.id).await.unwrap();
        let after_stop = arg_lines(&std::fs::read_to_string(&log).unwrap());
        assert_eq!(count_arg(&after_stop, "kill"), 3);
        assert_eq!(count_arg(&after_stop, "SIGTERM"), 3);
        let kill_ids: Vec<String> = after_stop
            .windows(3)
            .filter(|w| w[0] == "kill" && w[2] == "SIGTERM")
            .map(|w| w[1].clone())
            .collect();
        assert_eq!(
            kill_ids,
            vec![
                format!("{}-1", request.id),
                format!("{}-0", request.id),
                request.id.to_string(),
            ]
        );
        assert_eq!(rt.list().await.unwrap()[0].power, PowerState::Off);

        let missing = tmp.join("missing-youki");
        std::env::set_var("PISTACK_YOUKI_BIN", &missing);
        for err in [
            rt.start(&request).await.unwrap_err(),
            rt.stop(&request.id).await.unwrap_err(),
            rt.delete(&request.id).await.unwrap_err(),
            rt.list().await.unwrap_err(),
        ] {
            assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        }
        std::env::set_var("PISTACK_YOUKI_BIN", &bin);

        let before_bad = std::fs::read_to_string(&log).unwrap();
        let named = req("named", "nginx -g daemon off;", "10.2.0.7");
        let err = rt.start(&named).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("image cannot be unpacked"), "{err}");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), before_bad);
        assert!(rt.list().await.unwrap().iter().all(|m| m.id != named.id));

        let empty = tmp.join("empty-rootfs");
        std::fs::create_dir(&empty).unwrap();
        let empty_req = req("empty", empty.to_str().unwrap(), "10.2.0.8");
        let err = rt.start(&empty_req).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("image rootfs is empty"), "{err}");
        assert!(!state
            .join(empty_req.id.to_string())
            .join("state.json")
            .exists());
        assert!(rt
            .list()
            .await
            .unwrap()
            .iter()
            .all(|m| m.id != empty_req.id));

        let link = tmp.join("link-rootfs");
        std::os::unix::fs::symlink(&external, &link).unwrap();
        let link_req = req("link", link.to_str().unwrap(), "10.2.0.9");
        let err = rt.start(&link_req).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("image cannot be unpacked"), "{err}");

        let dotted = format!(
            "{}/../{}/external-rootfs",
            tmp.parent().unwrap().display(),
            tmp.file_name().unwrap().to_string_lossy()
        );
        assert!(dotted.contains(".."));
        let dot_req = req("dotted", &dotted, "10.2.0.10");
        let err = rt.start(&dot_req).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert_eq!(
            std::fs::read_to_string(external.join("marker")).unwrap(),
            "keep"
        );

        let archive = tmp.join("saved.tar");
        std::fs::write(&archive, docker_save()).unwrap();
        let tar_req = req("saved", archive.to_str().unwrap(), "10.2.0.11");
        rt.start(&tar_req).await.unwrap();
        let tar_bundle = state.join(tar_req.id.to_string());
        let tar_cfg: serde_json::Value =
            serde_json::from_slice(&std::fs::read(tar_bundle.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(
            tar_cfg["process"]["args"],
            serde_json::json!(["/bin/app", "serve"])
        );
        assert_eq!(tar_cfg["process"]["cwd"], "/app");
        let tar_root = tar_bundle.join("rootfs");
        assert_eq!(
            std::fs::read_to_string(tar_root.join("keep")).unwrap(),
            "yes"
        );
        assert!(!tar_root.join("gone").exists());
        assert!(!tar_root.join("sub").join("old").exists());
        assert_eq!(
            std::fs::read_to_string(tar_root.join("sub").join("new")).unwrap(),
            "n"
        );
        let tar_text = std::fs::read_to_string(tar_bundle.join("config.json")).unwrap()
            + &std::fs::read_to_string(tar_bundle.join("state.json")).unwrap()
            + &tree_text(&tar_root);
        assert!(!tar_text.contains("do-not-log-user-data"));
        assert!(!tar_text.contains("do-not-log-ssh-key"));
        let tar_log = std::fs::read_to_string(&log).unwrap();
        assert!(!tar_log.contains(archive.to_str().unwrap()));

        write_script(&bin, &log, 1);
        let bad_root = tmp.join("bad-rootfs");
        std::fs::create_dir(&bad_root).unwrap();
        std::fs::write(bad_root.join("marker"), "keep").unwrap();
        let bad = req("bad", bad_root.to_str().unwrap(), "10.3.0.1");
        let err = rt.start(&bad).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("youki exploded"), "{}", err.message);
        assert!(!state.join(bad.id.to_string()).join("state.json").exists());
        assert!(rt
            .list()
            .await
            .unwrap()
            .iter()
            .all(|m| m.id != bad.id || m.power != PowerState::On));

        write_script(&bin, &log, 0);
        rt.delete(&bad.id).await.unwrap();
        rt.delete(&tar_req.id).await.unwrap();
        rt.delete(&request.id).await.unwrap();
        rt.delete(&request.id).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(external.join("marker")).unwrap(),
            "keep"
        );
        assert!(external.join("bin").join("web").is_file());
        assert_eq!(
            rt.stop(&request.id).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert!(rt.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn unpacks_mold_oci_and_rejects_slip_and_unknown_compression() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-unpack-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let prev_mold = std::env::var_os("PISTACK_MOLD_DIR");
        std::env::set_var("PISTACK_MOLD_DIR", &tmp);

        let hex = format!("ab{}", "cd".repeat(31));
        assert_eq!(hex.len(), 64);
        let blob = tmp.join("blobs").join("sha256").join(&hex[..2]).join(&hex);
        std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
        std::fs::write(&blob, tar_file(&[("mold-marker", b"mold")])).unwrap();
        let found = digest_image(&format!("sha256:{hex}")).unwrap();
        assert_eq!(found, blob);
        let mold_root = tmp.join("mold-root");
        std::fs::create_dir(&mold_root).unwrap();
        unpack_image(&found, &mold_root).unwrap();
        assert_eq!(
            std::fs::read_to_string(mold_root.join("mold-marker")).unwrap(),
            "mold"
        );

        let manifest = "aa".repeat(32);
        let config = "bb".repeat(32);
        let layer = "cc".repeat(32);
        let layout = tmp.join("oci");
        let blobs = layout.join("blobs").join("sha256");
        std::fs::create_dir_all(&blobs).unwrap();
        std::fs::write(
            layout.join("oci-layout"),
            b"{\"imageLayoutVersion\":\"1.0.0\"}",
        )
        .unwrap();
        std::fs::write(
            layout.join("index.json"),
            format!(r#"{{"manifests":[{{"digest":"sha256:{manifest}"}}]}}"#),
        )
        .unwrap();
        std::fs::write(
            blobs.join(&manifest),
            format!(
                r#"{{"config":{{"digest":"sha256:{config}"}},"layers":[{{"mediaType":"application/vnd.oci.image.layer.v1.tar","digest":"sha256:{layer}"}}]}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            blobs.join(&config),
            br#"{"config":{"Entrypoint":["/bin/from-oci"],"Cmd":["run"],"Env":["FROM_OCI=1"],"WorkingDir":"/work"}}"#,
        )
        .unwrap();
        std::fs::write(blobs.join(&layer), tar_file(&[("oci-marker", b"oci")])).unwrap();
        let oci_root = tmp.join("oci-root");
        std::fs::create_dir(&oci_root).unwrap();
        let cfg = unpack_image(&layout, &oci_root).unwrap();
        assert_eq!(cfg.entrypoint, vec!["/bin/from-oci".to_string()]);
        assert_eq!(cfg.working_dir, "/work");
        assert_eq!(
            std::fs::read_to_string(oci_root.join("oci-marker")).unwrap(),
            "oci"
        );

        let saved = tmp.join("saved.tar");
        std::fs::write(&saved, docker_save()).unwrap();
        let docker_root = tmp.join("docker-root");
        std::fs::create_dir(&docker_root).unwrap();
        let cfg = unpack_image(&saved, &docker_root).unwrap();
        assert_eq!(cfg.entrypoint, vec!["/bin/app".to_string()]);
        assert_eq!(cfg.cmd, vec!["serve".to_string()]);
        assert_eq!(cfg.working_dir, "/app");
        assert_eq!(
            std::fs::read_to_string(docker_root.join("keep")).unwrap(),
            "yes"
        );
        assert!(!docker_root.join("gone").exists());
        assert!(!docker_root.join("sub").join("old").exists());
        assert_eq!(
            std::fs::read_to_string(docker_root.join("sub").join("new")).unwrap(),
            "n"
        );

        let slip = tmp.join("slip.tar");
        std::fs::write(&slip, tar_file(&[("../escape", b"no")])).unwrap();
        let slip_root = tmp.join("slip-root");
        std::fs::create_dir(&slip_root).unwrap();
        let err = unpack_image(&slip, &slip_root).unwrap_err();
        assert!(err.message.contains("image cannot be unpacked"), "{err}");
        assert!(!tmp.join("escape").exists());
        assert!(!slip_root.join("escape").exists());

        std::fs::write(tmp.join("zstd.bin"), [0x28, 0xb5, 0x2f, 0xfd, 0, 0, 0, 0]).unwrap();
        let err = unpack_image(&tmp.join("zstd.bin"), &slip_root).unwrap_err();
        assert!(
            err.message
                .contains("image layer compression is not available"),
            "{err}"
        );

        std::fs::write(tmp.join("empty-file"), b"").unwrap();
        assert!(unpack_image(&tmp.join("empty-file"), &slip_root).is_err());
        std::os::unix::fs::symlink(&saved, tmp.join("link.tar")).unwrap();
        let err = unpack_image(&tmp.join("link.tar"), &slip_root).unwrap_err();
        assert!(err.message.contains("image cannot be unpacked"), "{err}");

        let src = tmp.join("plain");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("marker"), "keep").unwrap();
        let outside = tmp.join("outside");
        std::fs::write(&outside, "secret-body").unwrap();
        std::os::unix::fs::symlink(&outside, src.join("abs")).unwrap();
        std::os::unix::fs::symlink("marker", src.join("rel")).unwrap();
        let copied = tmp.join("copied");
        std::fs::create_dir(&copied).unwrap();
        unpack_image(&src, &copied).unwrap();
        assert_eq!(
            std::fs::read_to_string(copied.join("marker")).unwrap(),
            "keep"
        );
        assert!(std::fs::symlink_metadata(copied.join("abs"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_link(copied.join("abs")).unwrap(), outside);
        assert_ne!(
            std::fs::read_to_string(copied.join("marker")).unwrap(),
            "secret-body"
        );

        let empty = tmp.join("empty-dir");
        std::fs::create_dir(&empty).unwrap();
        let err = install_bundle(
            &tmp.join("empty-bundle"),
            &empty,
            "h",
            &Limits {
                vcpus: 1,
                memory_mib: 1,
            },
            None,
            &RunMode::privileged(),
        )
        .unwrap_err();
        assert!(err.message.contains("image rootfs is empty"), "{err}");

        match prev_mold {
            Some(v) => std::env::set_var("PISTACK_MOLD_DIR", v),
            None => std::env::remove_var("PISTACK_MOLD_DIR"),
        }
    }

    #[test]
    fn resolves_env_from_config_map_and_secret() {
        let mut app = Projected::default();
        app.data.insert("MODE".into(), "from-data".into());
        app.binary_data.insert("MODE".into(), "aWdub3JlZA==".into());
        app.binary_data.insert("token".into(), "c2VjcmV0".into());
        app.binary_data.insert("raw".into(), "/w==".into());
        let mut db = Projected::default();
        db.data.insert("password".into(), "s3cret-value".into());
        let mut config_maps = BTreeMap::new();
        config_maps.insert("app".into(), app);
        let mut secrets = BTreeMap::new();
        secrets.insert("db".into(), db);
        let env: Vec<SandboxEnv> = serde_json::from_value(serde_json::json!([
            {"name": "APP_MODE", "value": "prod"},
            {"name": "MODE", "value": "nope", "value_from": {"config_map": {"name": "app", "key": "MODE"}}},
            {"name": "TOKEN", "value_from": {"config_map": {"name": "app", "key": "token"}}},
            {"name": "PASSWORD", "value": "nope", "value_from": {"secret": {"name": "db", "key": "password"}}},
            {"name": "MISSING", "value": "nope", "value_from": {"secret": {"name": "db", "key": "nope"}}},
            {"name": "RAW", "value": "nope", "value_from": {"config_map": {"name": "app", "key": "raw"}}},
            {"name": "BAD", "value": "do-not-log-user-data", "value_from": {"secret": "do-not-log-user-data"}},
            {"name": "", "value": "skip"}
        ]))
        .unwrap();
        let resolved = resolve_process_env(
            &["FROM=image".into(), "APP_MODE=image".into()],
            &env,
            &config_maps,
            &secrets,
        );
        assert!(resolved
            .iter()
            .any(|e| e == &format!("PATH={DEFAULT_PATH}")));
        assert!(resolved.iter().any(|e| e == "FROM=image"));
        assert!(resolved.iter().any(|e| e == "APP_MODE=prod"));
        assert!(resolved.iter().any(|e| e == "MODE=from-data"));
        assert!(resolved.iter().any(|e| e == "TOKEN=secret"));
        assert!(resolved.iter().any(|e| e == "PASSWORD=s3cret-value"));
        assert!(resolved.iter().all(|e| !e.starts_with("MISSING=")));
        assert!(resolved.iter().all(|e| !e.starts_with("RAW=")));
        assert!(resolved.iter().all(|e| !e.starts_with("BAD=")));
        assert!(resolved.iter().all(|e| !e.contains("do-not-log")));
        assert_eq!(
            parse_state_pid(br#"{"pid":4242,"status":"running"}"#),
            Some(4242)
        );
        assert_eq!(parse_state_pid(br#"{"pid":0}"#), None);
        assert_eq!(parse_state_pid(br"not json"), None);
    }

    #[test]
    fn device_names_and_host_paths() {
        assert!(host_path_ok("/var/lib/pistack/vol").is_ok());
        assert_eq!(host_path_ok("rel").unwrap_err().code, ErrorCode::BadRequest);
        assert!(host_path_ok("").is_err());
        assert!(host_path_ok("/tmp/../etc").is_err());
        assert!(host_path_ok("/ok\0").is_err());
        assert!(check_device_name("vdb").is_ok());
        assert!(check_device_name("vdz").is_ok());
        assert!(check_device_name("vda").is_err());
        assert!(check_device_name("vd").is_err());
        assert!(check_device_name("/dev/vdb").is_err());
        let mut disks = Vec::new();
        for c in b'b'..=b'z' {
            let name = next_device(&disks).unwrap();
            assert_eq!(name, format!("vd{}", c as char));
            disks.push(AttachedDisk {
                volume: format!("vol-{c}"),
                path: "/dev/null".into(),
                device: name,
                read_only: false,
            });
        }
        assert_eq!(
            next_device(&disks).unwrap_err().code,
            ErrorCode::Unprocessable
        );
        assert_eq!(PAUSE_ELF.len(), 154);
        assert_eq!(&PAUSE_ELF[..4], b"\x7fELF");
        assert_eq!(&PAUSE_ELF[24..32], &0x400078u64.to_le_bytes());
    }

    #[tokio::test]
    async fn pod_bundles_mount_every_container() {
        let _guard = env_lock().lock().await;
        let _netns = EnvSlot::clear(NETNS_ENV);
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-pod-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let image_a = tmp.join("image-a");
        let image_b = tmp.join("image-b");
        std::fs::create_dir_all(image_a.join("bin")).unwrap();
        std::fs::create_dir_all(&image_b).unwrap();
        std::fs::write(image_a.join("marker"), "a").unwrap();
        std::fs::write(image_b.join("marker"), "b").unwrap();
        let disk = tmp.join("slab-file");
        std::fs::write(&disk, "block").unwrap();
        let vol_id = "vol-11111111-1111-1111-1111-111111111111";
        let user_data = serde_json::json!({
            "kind": "PodSandbox",
            "containers": [
                {
                    "name": "web",
                    "image": image_a,
                    "command": ["/usr/bin/web"],
                    "args": ["--serve"],
                    "working_dir": "/work",
                    "env": [
                        {"name": "PASSWORD", "value_from": {"secret": {"name": "db", "key": "password"}}}
                    ],
                    "volume_mounts": [
                        {"name": "scratch", "mount_path": "/tmp/scratch"},
                        {"name": "cfg", "mount_path": "/etc/cfg", "read_only": true},
                        {"name": "cfg", "mount_path": "/etc/hello", "sub_path": "hello", "read_only": true},
                        {"name": "sec", "mount_path": "/run/sec", "read_only": true},
                        {"name": "disk", "mount_path": "/data"},
                        {"name": "scratch", "mount_path": "/tmp/../escape"},
                        {"name": "cfg", "mount_path": "/etc/slip", "sub_path": "../escape"}
                    ]
                },
                {
                    "name": "side",
                    "image": image_b,
                    "command": ["/usr/bin/side"],
                    "args": ["--watch"],
                    "volume_mounts": [
                        {"name": "scratch", "mount_path": "/cache", "read_only": true}
                    ]
                }
            ],
            "volumes": [
                {"name": "scratch", "source": "empty_dir"},
                {"name": "cfg", "source": {"config_map": "app"}},
                {"name": "sec", "source": {"secret": "db"}},
                {"name": "disk", "source": {"volume": vol_id}}
            ],
            "config_maps": {
                "app": {
                    "data": {"MODE": "prod", "hello": "world"},
                    "binary_data": {"MODE": "aWdub3JlZA==", "blob": "Ymlu"}
                }
            },
            "secrets": {
                "db": {"data": {"password": "s3cret-value"}}
            }
        })
        .to_string();
        let mut request = req("web", "not-a-path", "10.9.0.1");
        request.user_data = Some(user_data);
        let plan = sandbox_plan(&request).unwrap().unwrap();
        let bundle = tmp.join("pod");
        let limits = Limits {
            vcpus: 1,
            memory_mib: 64,
        };
        let privileged = RunMode::privileged();
        privileged
            .stage_pod_bundles(&bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();
        let scratch = bundle.join("volumes").join("0-scratch");
        std::fs::write(scratch.join("keep-me"), "yes").unwrap();
        privileged
            .stage_pod_bundles(&bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(scratch.join("keep-me")).unwrap(),
            "yes"
        );

        let cfg_dir = bundle.join("volumes").join("1-cfg");
        let sec_dir = bundle.join("volumes").join("2-sec");
        assert_eq!(
            std::fs::read_to_string(cfg_dir.join("MODE")).unwrap(),
            "prod"
        );
        assert_eq!(
            std::fs::read_to_string(cfg_dir.join("hello")).unwrap(),
            "world"
        );
        assert_eq!(
            std::fs::read_to_string(cfg_dir.join("blob")).unwrap(),
            "bin"
        );
        assert_eq!(mode_of(&cfg_dir.join("MODE")), 0o644);
        assert_eq!(
            std::fs::read_to_string(sec_dir.join("password")).unwrap(),
            "s3cret-value"
        );
        assert_eq!(mode_of(&sec_dir.join("password")), 0o400);
        assert_eq!(mode_of(&sec_dir), 0o700);
        std::fs::write(cfg_dir.join("extra"), "gone").unwrap();
        privileged
            .stage_pod_bundles(&bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();
        assert!(!cfg_dir.join("extra").exists());
        assert_eq!(
            std::fs::read_to_string(scratch.join("keep-me")).unwrap(),
            "yes"
        );

        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let side: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("1").join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            web["process"]["args"],
            serde_json::json!(["/usr/bin/web", "--serve"])
        );
        assert_eq!(
            side["process"]["args"],
            serde_json::json!(["/usr/bin/side", "--watch"])
        );
        assert_eq!(
            ns_path(&web, "network").as_deref(),
            Some("/proc/4242/ns/net")
        );
        assert_eq!(
            ns_path(&side, "network").as_deref(),
            Some("/proc/4242/ns/net")
        );
        assert!(web.get("hostname").is_none());
        let sandbox: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bundle.join("config.json")).unwrap()).unwrap();
        assert!(sandbox["linux"]["namespaces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n.get("path").is_none()));
        assert_eq!(
            std::fs::read_to_string(
                bundle
                    .join("containers")
                    .join("0")
                    .join("rootfs")
                    .join("marker")
            )
            .unwrap(),
            "a"
        );
        assert_eq!(
            std::fs::read_to_string(
                bundle
                    .join("containers")
                    .join("1")
                    .join("rootfs")
                    .join("marker")
            )
            .unwrap(),
            "b"
        );
        let env = web["process"]["env"].as_array().unwrap();
        assert!(env.iter().any(|v| v == "PASSWORD=s3cret-value"));
        assert!(web["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["destination"] != "/data"));
        let scratch_mount = mount_at(&web, "/tmp/scratch");
        assert_eq!(scratch_mount["source"], scratch.to_string_lossy().as_ref());
        assert_eq!(
            scratch_mount["options"],
            serde_json::json!(["rbind", "rprivate", "rw"])
        );
        let hello = mount_at(&web, "/etc/hello");
        assert_eq!(
            hello["source"],
            cfg_dir.join("hello").to_string_lossy().as_ref()
        );
        assert_eq!(
            hello["options"],
            serde_json::json!(["bind", "rprivate", "ro"])
        );
        let sec = mount_at(&web, "/run/sec");
        assert_eq!(
            sec["options"],
            serde_json::json!(["rbind", "rprivate", "ro"])
        );
        let cache = mount_at(&side, "/cache");
        assert_eq!(cache["source"], scratch.to_string_lossy().as_ref());
        assert_eq!(
            cache["options"],
            serde_json::json!(["rbind", "rprivate", "ro"])
        );
        assert!(web["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["destination"] != "/tmp/../escape" && m["destination"] != "/etc/slip"));
        let plan_text = std::fs::read_to_string(bundle.join("mount-plan.json")).unwrap();
        assert!(!plan_text.contains("s3cret-value"));
        assert!(!plan_text.contains("do-not-log-user-data"));
        assert!(!std::fs::read_to_string(bundle.join("config.json"))
            .unwrap()
            .contains("s3cret-value"));

        write_disks(
            &bundle,
            &[AttachedDisk {
                volume: vol_id.into(),
                path: disk.to_string_lossy().into_owned(),
                device: "vdb".into(),
                read_only: true,
            }],
        )
        .unwrap();
        refresh_disk_mounts(&bundle).unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], disk.to_string_lossy().as_ref());
        assert_eq!(
            data["options"],
            serde_json::json!(["bind", "rprivate", "ro"])
        );
        assert_eq!(
            web["process"]["args"],
            serde_json::json!(["/usr/bin/web", "--serve"])
        );
        assert_eq!(
            mode_of(&bundle.join("containers").join("0").join("config.json")),
            0o600
        );
    }

    fn mount_at<'a>(cfg: &'a serde_json::Value, dest: &str) -> &'a serde_json::Value {
        cfg["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["destination"] == dest)
            .unwrap()
    }

    #[tokio::test]
    async fn attach_is_not_found_until_the_bundle_exists() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-attach-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let _env = EnvGuard::set(&tmp.join("youki"), &tmp.join("state"));
        let rt = YoukiRuntime;
        let machine = Id::new(kind::MACHINE);
        let volume = Id::new(kind::VOLUME);
        let attach = crate::runtime::VolumeAttach {
            machine: machine.clone(),
            volume: volume.clone(),
            path: "/var/lib/pistack/volumes/disk".into(),
            device: None,
            read_only: false,
            target: None,
        };
        assert_eq!(
            rt.attach_volume(&attach).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        let bundle = tmp.join("state").join(machine.to_string());
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("config.json"), b"{}").unwrap();
        let mut bad = attach.clone();
        bad.path = "relative".into();
        assert_eq!(
            rt.attach_volume(&bad).await.unwrap_err().code,
            ErrorCode::BadRequest
        );
        assert_eq!(rt.attach_volume(&attach).await.unwrap(), "vdb");
        let mut moved = attach.clone();
        moved.path = "/var/lib/pistack/volumes/disk2".into();
        assert_eq!(rt.attach_volume(&moved).await.unwrap(), "vdb");
        let mut second = attach.clone();
        second.volume = Id::new(kind::VOLUME);
        assert_eq!(rt.attach_volume(&second).await.unwrap(), "vdc");
        let mut named = attach.clone();
        named.volume = Id::new(kind::VOLUME);
        named.device = Some("vdk".into());
        assert_eq!(rt.attach_volume(&named).await.unwrap(), "vdk");
        named.device = Some("vda".into());
        named.volume = Id::new(kind::VOLUME);
        assert_eq!(
            rt.attach_volume(&named).await.unwrap_err().code,
            ErrorCode::BadRequest
        );
        rt.detach_volume(&crate::runtime::VolumeDetach {
            machine: machine.clone(),
            volume,
            path: attach.path.clone(),
            device: None,
            target: None,
        })
        .await
        .unwrap();
        let disks = read_disks(&bundle).unwrap();
        assert!(disks.iter().all(|d| d.device != "vdb"));
        assert_eq!(disks.len(), 2);
        rt.detach_volume(&crate::runtime::VolumeDetach {
            machine,
            volume: second.volume,
            path: second.path,
            device: None,
            target: None,
        })
        .await
        .unwrap();
    }

    fn write_exec_script(path: &Path, log: &Path) {
        let quoted = format!("'{}'", log.display().to_string().replace('\'', "'\\''"));
        let body = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$@\" >> {quoted}\n\
             printf '\\n' >> {quoted}\n\
             if [ \"$1\" = state ]; then\n\
               printf '%s\\n' '{{\"pid\":4242,\"status\":\"running\"}}'\n\
               exit 0\n\
             fi\n\
             if [ \"$1\" = exec ]; then\n\
               for arg in \"$@\"; do\n\
                 if [ \"$arg\" = __sleep__ ]; then\n\
                   sleep 30\n\
                   exit 0\n\
                 fi\n\
                 if [ \"$arg\" = __flood__ ]; then\n\
                   i=0\n\
                   while [ \"$i\" -lt 200 ]; do\n\
                     printf x\n\
                     i=$((i + 1))\n\
                   done\n\
                   i=0\n\
                   while [ \"$i\" -lt 80 ]; do\n\
                     printf y >&2\n\
                     i=$((i + 1))\n\
                   done\n\
                   exit 0\n\
                 fi\n\
               done\n\
               printf 'out-line\\n'\n\
               printf 'err-line\\n' >&2\n\
               exit 7\n\
             fi\n\
             exit 0\n"
        );
        std::fs::write(path, body).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    fn has_seq(lines: &[String], seq: &[String]) -> bool {
        lines.windows(seq.len()).any(|window| window == seq)
    }

    #[test]
    fn exec_output_trims_a_split_utf8_sequence() {
        assert_eq!(bytes_to_text("é".as_bytes()), "é");
        assert_eq!(bytes_to_text(&[0xc3]), "");
        assert_eq!(bytes_to_text(&[0xc3, 0xa9, 0xc3]), "é");
        assert_eq!(bytes_to_text(b"ok"), "ok");
    }

    #[tokio::test]
    async fn exec_runs_youki_with_argv_cap_and_timeout() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-exec-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let log = tmp.join("youki.log");
        let bin = tmp.join("youki");
        let state = tmp.join("state");
        write_exec_script(&bin, &log);
        let _env = EnvGuard::set(&bin, &state);
        let rt = YoukiRuntime;

        let external = tmp.join("image");
        std::fs::create_dir_all(&external).unwrap();
        std::fs::write(external.join("marker"), "keep").unwrap();
        let image = external.to_str().unwrap();
        let mut request = req("web", "not-a-path", "10.1.0.8");
        request.user_data = Some(sandbox_doc(image));

        let absent = Id::new(kind::MACHINE);
        let err = rt
            .exec(
                &absent,
                "web",
                &["/bin/true".into()],
                &[],
                None,
                Duration::from_secs(2),
                64,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound, "{err}");
        assert!(err.message.contains("not on this host"), "{err}");

        rt.start(&request).await.unwrap();
        let out = rt
            .exec(
                &request.id,
                "web",
                &["/bin/echo".into(), "hello".into()],
                &[
                    ("MODE".into(), "prod".into()),
                    ("EMPTY".into(), String::new()),
                ],
                Some("/work"),
                Duration::from_secs(5),
                EXEC_OUTPUT_CAP,
            )
            .await
            .unwrap();
        assert_eq!(out.exit_code, 7);
        assert_eq!(out.stdout, "out-line\n");
        assert_eq!(out.stderr, "err-line\n");
        let id0 = format!("{}-0", request.id);
        let lines = arg_lines(&std::fs::read_to_string(&log).unwrap());
        assert!(
            has_seq(
                &lines,
                &[
                    "exec".into(),
                    "--cwd".into(),
                    "/work".into(),
                    "--env".into(),
                    "MODE=prod".into(),
                    "--env".into(),
                    "EMPTY=".into(),
                    id0.clone(),
                    "--".into(),
                    "/bin/echo".into(),
                    "hello".into(),
                ],
            ),
            "{lines:?}"
        );

        let side = rt
            .exec(
                &request.id,
                "side",
                &["/bin/true".into()],
                &[],
                None,
                Duration::from_secs(5),
                64,
            )
            .await
            .unwrap();
        assert_eq!(side.exit_code, 7);
        let id1 = format!("{}-1", request.id);
        let lines = arg_lines(&std::fs::read_to_string(&log).unwrap());
        assert!(
            has_seq(
                &lines,
                &["exec".into(), id1, "--".into(), "/bin/true".into(),],
            ),
            "{lines:?}"
        );

        let by_id = rt
            .exec(
                &request.id,
                &id0,
                &["/bin/true".into()],
                &[],
                None,
                Duration::from_secs(5),
                64,
            )
            .await
            .unwrap();
        assert_eq!(by_id.exit_code, 7);

        let flooded = rt
            .exec(
                &request.id,
                "web",
                &["__flood__".into()],
                &[],
                None,
                Duration::from_secs(5),
                16,
            )
            .await
            .unwrap();
        assert_eq!(flooded.exit_code, 0);
        assert_eq!(flooded.stdout, "x".repeat(16));
        assert_eq!(flooded.stderr, "y".repeat(16));

        let started = std::time::Instant::now();
        let err = rt
            .exec(
                &request.id,
                "web",
                &["__sleep__".into()],
                &[],
                None,
                Duration::from_millis(400),
                64,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("timed out"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "exec timeout took {:?}",
            started.elapsed()
        );

        let before = count_arg(&arg_lines(&std::fs::read_to_string(&log).unwrap()), "exec");
        let err = rt
            .exec(
                &request.id,
                "nope",
                &["/bin/true".into()],
                &[],
                None,
                Duration::from_secs(2),
                64,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound, "{err}");
        assert!(err.message.contains("nope"), "{err}");
        let err = rt
            .exec(
                &request.id,
                "web",
                &[],
                &[],
                None,
                Duration::from_secs(2),
                64,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest, "{err}");
        assert!(err.message.contains("empty"), "{err}");
        let after = count_arg(&arg_lines(&std::fs::read_to_string(&log).unwrap()), "exec");
        assert_eq!(before, after, "a rejected exec must not call youki");

        rt.stop(&request.id).await.unwrap();
        let err = rt
            .exec(
                &request.id,
                "web",
                &["/bin/true".into()],
                &[],
                None,
                Duration::from_secs(2),
                64,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("not running"), "{err}");

        let missing_bin = tmp.join("missing-youki");
        std::env::set_var("PISTACK_YOUKI_BIN", &missing_bin);
        let err = rt
            .exec(
                &request.id,
                "web",
                &["/bin/true".into()],
                &[],
                None,
                Duration::from_secs(2),
                64,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("not available"), "{err}");

        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(!logged.contains("do-not-log-user-data"), "{logged}");
        assert!(!logged.contains("do-not-log-ssh-key"), "{logged}");
    }

    fn ns_kinds(cfg: &serde_json::Value) -> Vec<&str> {
        cfg["linux"]["namespaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|ns| ns["type"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn root_and_sudo_configs_stay_privileged() {
        let limits = Limits {
            vcpus: 2,
            memory_mib: 64,
        };
        let args = vec!["/bin/app".to_string()];
        let env = vec!["PATH=/bin".to_string()];
        for mode in [
            RunMode::privileged(),
            RunMode {
                privilege: Privilege::Sudo {
                    sudo: PathBuf::from("/bin/sudo"),
                },
                cgroup: CgroupCaps {
                    memory: true,
                    cpu: true,
                },
            },
        ] {
            let spec = mode.oci_config(
                Some("host"),
                &limits,
                (&args, &env, "/"),
                &sandbox_namespaces(&mode, None),
                &[],
            );
            assert_eq!(spec["process"]["user"]["uid"], 0);
            assert_eq!(spec["process"]["user"]["gid"], 0);
            assert!(spec["linux"].get("uidMappings").is_none());
            assert!(spec["linux"].get("gidMappings").is_none());
            assert_eq!(
                ns_kinds(&spec),
                vec!["pid", "network", "ipc", "uts", "mount"]
            );
            assert!(spec["linux"]["namespaces"]
                .as_array()
                .unwrap()
                .iter()
                .all(|ns| ns.get("path").is_none()));
            assert_eq!(
                spec["linux"]["resources"]["memory"]["limit"],
                64 * 1024 * 1024
            );
            assert_eq!(spec["linux"]["resources"]["cpu"]["period"], 100_000);
            assert_eq!(spec["linux"]["resources"]["cpu"]["quota"], 200_000);
        }
    }

    #[tokio::test]
    async fn rootless_pod_maps_one_id_and_joins_the_sandbox_user_namespace() {
        let _guard = env_lock().lock().await;
        let _netns = EnvSlot::clear(NETNS_ENV);
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-rootless-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let image = tmp.join("image");
        std::fs::create_dir_all(image.join("bin")).unwrap();
        std::fs::write(image.join("marker"), "a").unwrap();
        let mut request = req("web", "not-a-path", "10.9.0.2");
        request.user_data = Some(sandbox_doc(image.to_str().unwrap()));
        let plan = sandbox_plan(&request).unwrap().unwrap();
        let bundle = tmp.join("pod");
        let limits = Limits {
            vcpus: 2,
            memory_mib: 64,
        };
        let mode = RunMode::rootless(1000, 1000);
        mode.stage_pod_bundles(&bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();

        let sandbox: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bundle.join("config.json")).unwrap()).unwrap();
        assert_eq!(
            ns_kinds(&sandbox),
            vec!["user", "pid", "network", "ipc", "uts", "mount"]
        );
        assert!(sandbox["linux"]["namespaces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|ns| ns.get("path").is_none()));
        assert_eq!(
            sandbox["linux"]["uidMappings"],
            serde_json::json!([{ "containerID": 0, "hostID": 1000, "size": 1 }])
        );
        assert_eq!(
            sandbox["linux"]["gidMappings"],
            serde_json::json!([{ "containerID": 0, "hostID": 1000, "size": 1 }])
        );
        assert_eq!(sandbox["process"]["user"]["uid"], 0);
        assert_eq!(sandbox["process"]["user"]["gid"], 0);
        assert!(sandbox["linux"].get("resources").is_none());

        for index in ["0", "1"] {
            let cfg: serde_json::Value = serde_json::from_slice(
                &std::fs::read(bundle.join("containers").join(index).join("config.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                ns_kinds(&cfg),
                vec!["user", "pid", "network", "ipc", "uts", "mount"]
            );
            assert_eq!(ns_path(&cfg, "user").as_deref(), Some("/proc/4242/ns/user"));
            assert_eq!(ns_path(&cfg, "pid").as_deref(), Some("/proc/4242/ns/pid"));
            assert_eq!(
                ns_path(&cfg, "network").as_deref(),
                Some("/proc/4242/ns/net")
            );
            assert_eq!(ns_path(&cfg, "ipc").as_deref(), Some("/proc/4242/ns/ipc"));
            assert_eq!(ns_path(&cfg, "uts").as_deref(), Some("/proc/4242/ns/uts"));
            assert_eq!(ns_path(&cfg, "mount"), None);
            assert!(cfg["linux"].get("uidMappings").is_none());
            assert!(cfg["linux"].get("gidMappings").is_none());
            assert_eq!(cfg["process"]["user"]["uid"], 0);
            assert!(cfg["linux"].get("resources").is_none());
        }

        let mut delegated = mode.clone();
        delegated.cgroup = CgroupCaps {
            memory: true,
            cpu: false,
        };
        delegated
            .stage_pod_bundles(&bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();
        let sandbox: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bundle.join("config.json")).unwrap()).unwrap();
        assert_eq!(
            sandbox["linux"]["resources"]["memory"]["limit"],
            64 * 1024 * 1024
        );
        assert!(sandbox["linux"]["resources"].get("cpu").is_none());

        let netns = tmp.join("netns");
        std::fs::write(&netns, b"ns").unwrap();
        let owner = file_uid(&netns).unwrap();
        let owned = RunMode::rootless(owner, owner);
        let joined = sandbox_namespaces(&owned, Some(&netns));
        assert_eq!(
            joined
                .iter()
                .find(|ns| ns["type"] == "network")
                .and_then(|ns| ns["path"].as_str()),
            Some(netns.to_str().unwrap())
        );
        let foreign = RunMode::rootless(owner.wrapping_add(1), owner);
        let skipped = sandbox_namespaces(&foreign, Some(&netns));
        assert!(skipped
            .iter()
            .find(|ns| ns["type"] == "network")
            .unwrap()
            .get("path")
            .is_none());
        assert_eq!(
            skipped.first().and_then(|ns| ns["type"].as_str()),
            Some("user")
        );
        let root = sandbox_namespaces(&RunMode::privileged(), Some(&netns));
        assert_eq!(
            root.iter()
                .find(|ns| ns["type"] == "network")
                .and_then(|ns| ns["path"].as_str()),
            Some(netns.to_str().unwrap())
        );
        assert!(root.iter().all(|ns| ns["type"] != "user"));
        assert!(host_netns_from(Some(std::ffi::OsStr::new("rel/ns"))).is_none());
        assert!(host_netns_from(Some(std::ffi::OsStr::new("/tmp/../etc"))).is_none());
        assert_eq!(
            host_netns_from(Some(std::ffi::OsStr::new("/var/run/netns/pod"))),
            Some(PathBuf::from("/var/run/netns/pod"))
        );
    }

    fn write_sudo(path: &Path, log: &Path) {
        let quoted = format!("'{}'", log.display().to_string().replace('\'', "'\\''"));
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {quoted}\nprintf '\\n' >> {quoted}\nif [ \"$1\" = -n ]; then shift; fi\nexec \"$@\"\n"
        );
        std::fs::write(path, body).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    fn assert_sudo_op(lines: &[String], youki: &str, op: &str) {
        assert!(
            lines
                .windows(3)
                .any(|w| w[0] == "-n" && w[1] == youki && w[2] == op),
            "missing sudo {op}: {lines:?}"
        );
    }

    #[tokio::test]
    async fn sudo_wraps_every_youki_command_and_root_ignores_it() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-sudo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let youki_log = tmp.join("youki.log");
        let sudo_log = tmp.join("sudo.log");
        let youki = tmp.join("youki");
        let sudo = tmp.join("sudo");
        write_script(&youki, &youki_log, 0);
        write_sudo(&sudo, &sudo_log);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        std::env::set_var("PISTACK_YOUKI_TEST_EUID", "1000");
        std::env::set_var("PISTACK_YOUKI_TEST_EGID", "1000");
        std::env::set_var(SUDO_ENV, "1");
        std::env::set_var(SUDO_BIN_ENV, &sudo);
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");

        let image = tmp.join("image");
        std::fs::create_dir_all(image.join("bin")).unwrap();
        std::fs::write(image.join("marker"), "keep").unwrap();
        let mut request = req("web", "not-a-path", "10.8.0.1");
        request.user_data = Some(sandbox_doc(image.to_str().unwrap()));
        let rt = YoukiRuntime;
        rt.start(&request).await.unwrap();
        let bundle = state.join(request.id.to_string());
        let cfg: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bundle.join("config.json")).unwrap()).unwrap();
        assert!(cfg["linux"].get("uidMappings").is_none());
        assert_eq!(
            ns_kinds(&cfg),
            vec!["pid", "network", "ipc", "uts", "mount"]
        );
        assert_eq!(cfg["process"]["user"]["uid"], 0);
        assert!(cfg["linux"]["resources"].get("memory").is_some());

        let youki_arg = youki.to_string_lossy().into_owned();
        let exec_out = rt
            .exec(
                &request.id,
                "web",
                &["/bin/echo".into(), "hello".into()],
                &[("MODE".into(), "prod".into())],
                Some("/work"),
                Duration::from_secs(5),
                EXEC_OUTPUT_CAP,
            )
            .await
            .unwrap();
        assert_eq!(exec_out.exit_code, 0);
        let sudo_lines = arg_lines(&std::fs::read_to_string(&sudo_log).unwrap());
        for op in ["create", "start", "state", "exec"] {
            assert_sudo_op(&sudo_lines, &youki_arg, op);
        }
        let youki_lines = arg_lines(&std::fs::read_to_string(&youki_log).unwrap());
        let web_id = format!("{}-0", request.id);
        assert!(
            has_seq(
                &youki_lines,
                &[
                    "exec".into(),
                    "--cwd".into(),
                    "/work".into(),
                    "--env".into(),
                    "MODE=prod".into(),
                    web_id,
                    "--".into(),
                    "/bin/echo".into(),
                    "hello".into(),
                ],
            ),
            "{youki_lines:?}"
        );

        // Rootless fallback runs youki directly, the same way as create and start.
        std::env::remove_var(SUDO_ENV);
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "1");
        let sudo_before = std::fs::read_to_string(&sudo_log).unwrap();
        let youki_before = std::fs::read_to_string(&youki_log).unwrap().len();
        let rootless = rt
            .exec(
                &request.id,
                "side",
                &["/bin/true".into()],
                &[],
                None,
                Duration::from_secs(5),
                64,
            )
            .await
            .unwrap();
        assert_eq!(rootless.exit_code, 0);
        assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), sudo_before);
        let youki_now = std::fs::read_to_string(&youki_log).unwrap();
        let added = arg_lines(&youki_now[youki_before..]);
        let side_id = format!("{}-1", request.id);
        assert!(
            has_seq(
                &added,
                &["exec".into(), side_id, "--".into(), "/bin/true".into(),],
            ),
            "{added:?}"
        );
        assert!(added.iter().all(|line| line != "-n"));
        std::env::set_var(SUDO_ENV, "1");
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");
        let youki_text = std::fs::read_to_string(&youki_log).unwrap();
        let sudo_text = std::fs::read_to_string(&sudo_log).unwrap();
        assert!(!youki_text.contains("do-not-log-user-data"));
        assert!(!youki_text.contains("do-not-log-ssh-key"));
        assert!(!sudo_text.contains("do-not-log-user-data"));
        assert!(!sudo_text.contains("do-not-log-ssh-key"));
        assert!(!youki_text.contains("-n"));

        rt.stop(&request.id).await.unwrap();
        rt.delete(&request.id).await.unwrap();
        let after = arg_lines(&std::fs::read_to_string(&sudo_log).unwrap());
        assert_sudo_op(&after, &youki_arg, "kill");
        assert_sudo_op(&after, &youki_arg, "delete");

        std::env::set_var("PISTACK_YOUKI_TEST_EUID", "0");
        let _ = std::fs::write(&sudo_log, b"");
        let _ = std::fs::write(&youki_log, b"");
        let plain = req("plain", image.to_str().unwrap(), "10.8.0.2");
        rt.start(&plain).await.unwrap();
        assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), "");
        let direct = arg_lines(&std::fs::read_to_string(&youki_log).unwrap());
        assert!(direct.iter().any(|line| line == "create"));
        assert!(direct.iter().all(|line| line != "-n"));
        rt.delete(&plain.id).await.unwrap();
        assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), "");
    }

    #[tokio::test]
    async fn privilege_errors_name_sudo_and_the_user_namespace() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-priv-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let youki = tmp.join("youki");
        write_script(&youki, &tmp.join("youki.log"), 0);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        std::env::set_var("PISTACK_YOUKI_TEST_EUID", "1000");
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");
        let rt = YoukiRuntime;
        let request = req("web", "not-a-path", "10.8.0.3");
        let err = rt.start(&request).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains(SUDO_ENV), "{err}");
        assert!(err.message.contains("NOPASSWD"), "{err}");
        assert!(err.message.contains("newuidmap"), "{err}");
        assert!(!state.join(request.id.to_string()).exists());

        std::env::set_var(SUDO_ENV, "yes");
        std::env::set_var(SUDO_BIN_ENV, tmp.join("missing-sudo"));
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "1");
        let err = rt.start(&request).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("sudo is not available"), "{err}");
        assert!(err.message.contains(&format!("unset {SUDO_ENV}")), "{err}");
        assert!(!state.join(request.id.to_string()).exists());

        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");
        let err = rt.start(&request).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("NOPASSWD"), "{err}");
        assert!(!err.message.contains("unset"), "{err}");
    }

    struct DirGuard(PathBuf);
    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn scopeguard_dir(path: &Path) -> DirGuard {
        DirGuard(path.to_path_buf())
    }
}
