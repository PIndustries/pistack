// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! youki (OCI) container runtime.
//!
//! One directory per machine under `$PISTACK_YOUKI_STATE`. When that is
//! unset, the directory is `$PISTACK_DATA_DIR/youki` if `PISTACK_DATA_DIR`
//! is set, otherwise `/var/lib/pistack/youki`. The agent user must be able
//! to create and write that directory. A pod is a sandbox bundle plus one
//! bundle per container, in spec order. The sandbox holds the pid, network,
//! ipc and uts namespaces; each container joins those and keeps its own
//! mount namespace so it has its own rootfs. `config.json` is the OCI spec,
//! `rootfs` is the unpacked image or the sandbox pause program, and
//! `state.json` is written after a successful start. The binary is
//! `$PISTACK_YOUKI_BIN` when that is set, otherwise `youki` on `PATH`.
//! `user_data` and ssh keys are not logged and are not placed on the youki
//! argv. A missing youki binary, an image that cannot be unpacked, or an
//! empty rootfs is unavailable and is not recorded as running. `exec` runs
//! `youki exec` in one container of a pod sandbox and returns that command's
//! exit code, stdout, and stderr. Command probes call that same helper, with
//! the probe timeout and a small output cap, and they do not pass an
//! environment.
//!
//! Unpacking a layer keeps the permission bits from each tar header, including
//! execute. A directory image is copied the same way: each regular file keeps
//! its mode, and a directory mode is applied after that directory's children
//! are on disk, so a read-only directory does not block the copy. Setuid and
//! setgid are kept only when the agent is root or sudo mode is on; otherwise
//! those two bits are cleared. A symlink whose target resolves outside the
//! rootfs is not followed.
//!
//! `youki create` and `youki start` do not inherit the agent's pipes. With
//! `process.terminal` false, the container init keeps youki's stdout and
//! stderr open while it waits for `start`, so waiting on those pipes for EOF
//! never returns. A workload container's stdout and stderr are an
//! agent-opened `log` file in that container's bundle (the file machine logs
//! are read from). The sandbox pause process uses `/dev/null`. youki's own
//! text goes to `--log` on a file the agent creates, and stderr is read back
//! after youki exits. The wait is only on youki's exit status. A create that
//! times out or fails runs `youki delete --force` and removes the bundle.
//! Before create, a container `youki state` still knows about, and that this
//! agent does not have recorded as running, is removed the same way. Delete
//! uses `youki delete --force` and then removes the bundle. In sudo mode a
//! bundle or rootfs the agent cannot unlink is removed through the cleanup
//! helper below.
//!
//! Privilege. A root agent keeps this root OCI config and runs `youki`
//! directly. `PISTACK_YOUKI_SUDO` does not change that.
//!
//! When the agent is not root and `PISTACK_YOUKI_SUDO` is `1`, `true`, or
//! `yes`, every youki invocation (`create`, `start`, `kill`, `delete`,
//! `state`, `exec`) is `sudo -n <youki> ...` and the OCI config stays the root
//! config. Files this agent writes (the bundle, the unpacked rootfs, volume
//! files, and logs) stay owned by the agent. youki runs as root and may leave
//! root-owned files under the bundle. Removing those, and replacing a rootfs
//! the agent can no longer unlink, is `sudo -n <helper> delete --path <path>`.
//! The helper is `scripts/pistack-youki-cleanup`, installed as
//! `/usr/local/sbin/pistack-youki-cleanup` (`PISTACK_YOUKI_CLEANUP` overrides
//! the path). The agent checks that the path is inside the youki state
//! directory and contains no `..` before it calls sudo. The helper checks
//! again, unlinks symlinks without following them, and refuses anything else.
//! As root it only removes paths under `/var/lib/pistack/youki`, the default
//! state directory. sudo resets the environment, so a custom
//! `PISTACK_YOUKI_STATE` cannot widen that directory. `sudo -n` fails closed
//! without a rule. Allow only the youki binary, that helper, `ip`, and
//! `ovs-vsctl`:
//!
//! ```text
//! # /etc/sudoers.d/pistack-youki
//! ubuntu ALL=(root) NOPASSWD: /usr/bin/youki, /usr/local/sbin/pistack-youki-cleanup, /usr/sbin/ip, /usr/bin/ovs-vsctl
//! ```
//!
//! Replace `ubuntu` and the paths with the agent user and the binaries on
//! the host. `ip` is often `/usr/sbin/ip`. Leave sudo's environment reset.
//!
//! In sudo mode the bundle directory is `0700`, so other host users cannot
//! traverse it. ConfigMap and Secret volume directories are `0755` and their
//! files are `0644`. Secret files are `0644` rather than `0640`: the
//! container user is uid 0 and gid 0, not the agent, and is not in the
//! agent's group, so group-read would still leave the file unreadable when
//! the container has no `CAP_DAC_OVERRIDE`. The private bundle directory is
//! what hides those files from other accounts on the host. Root and rootless
//! keep Secret files at `0400` and the Secret directory at `0700`.
//!
//! An RBD volume on a pod is mapped with `rbd device map`. The device path
//! (`/dev/rbd/<pool>/<image>` when that link exists, otherwise `/dev/rbdN`)
//! is stored so detach and pod delete can unmap it. It is not the container
//! bind. A VM does not map the image: libvirt keeps it as a network disk.
//! `PISTACK_RBD` selects the `rbd` binary. A non-root agent with
//! `PISTACK_RBD_SUDO=1` runs map and unmap as `sudo -n`. The sudoers line
//! allows only those commands. See the `rbdmap` module.
//!
//! Before `youki create`, each slab volume a container mounts must name the
//! host path recorded at attach. That path is the directory where the agent
//! mounted the volume. It must exist, be a directory, and be a mount point.
//! The container bind-mounts that directory. A missing path fails the
//! create. An empty directory left after a reboot or an unmount fails the
//! create too, so the pod does not write onto the host disk. The check
//! looks that path up in `/proc/self/mountinfo`. When that file cannot be
//! read, the folder's device number is compared with its parent's. A raw
//! file or block device is refused, including the backing file or mapped
//! device kept for detach. A volume that only names a device, with no
//! mounted directory yet, is left unbound. Rewriting `config.json` after
//! attach does not change this check.
//!
//! When the agent is not root and sudo mode is off, the sandbox creates a
//! user namespace and maps container uid 0 and gid 0 onto the agent's uid and
//! gid with size 1. That map does not need `newuidmap`. Each pod container
//! joins the sandbox user namespace by path first, then the shared pid,
//! network, ipc and uts namespaces. Cgroup memory and cpu limits are written
//! only when that controller is delegated and the cgroup directory is
//! writable. `PISTACK_YOUKI_NETNS`, when it is an absolute path, is joined as
//! the network namespace; a rootless agent skips that path when it is owned
//! by another uid and keeps the sandbox's own network namespace. That
//! variable wins over the wiring below.
//!
//! A pod whose ports name a network is placed in a named network namespace
//! before `youki create`. The host end of a veth is set master of the same
//! bridge a virtual machine tap would use. When the port binding `vif_type`
//! is `ovs`, that end is added to the binding's OVS bridge with `ovs-vsctl`
//! instead, and `del-port` runs before the link is deleted. The other end
//! is `eth0` (then `eth1`, ...) inside that namespace, with the recorded
//! MAC, address, and, when a gateway is recorded, a default route. A root
//! agent runs `ip` and `ovs-vsctl` directly. In sudo mode those commands
//! are `sudo -n <ip>` and `sudo -n <ovs-vsctl>`. A rootless agent cannot do
//! this: it keeps the sandbox network namespace and reports that the pod
//! has no network. A pod with no port attachment keeps the sandbox network
//! namespace. Stop, delete, and a failed create remove the veth and the
//! namespace. The next start creates the sandbox again so it joins the new
//! namespace. Names left by a crashed create are removed before the next
//! create.

use super::podnet::{self, PodNetwork};
use super::{MachineRequest, MachineRuntime, PowerState, RunningMachine};
use async_trait::async_trait;
use pistack_rpc::proto::{DiskTarget, ExecOutput};
use pistack_types::{kind, ApiError, ApiResult, Id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const TIMEOUT: Duration = Duration::from_secs(30);
/// Milliseconds. Tests set this so a hung `create` fails without waiting 30s.
const TIMEOUT_ENV: &str = "PISTACK_YOUKI_TIMEOUT_MS";
const DEFAULT_STATE: &str = "/var/lib/pistack/youki";

fn youki_timeout() -> Duration {
    match std::env::var(TIMEOUT_ENV) {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(ms) if ms > 0 => Duration::from_millis(ms),
            _ => TIMEOUT,
        },
        Err(_) => TIMEOUT,
    }
}
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
    /// Named network namespace created for this pod. Empty when the sandbox
    /// namespace was left alone.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    netns: String,
    /// Host ends of the veth pairs. Empty when `netns` is empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    veths: Vec<String>,
    /// OVS ports removed before those veths. Empty on a Linux bridge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ovs: Vec<podnet::OvsLink>,
    /// Set when a rootless agent could not attach the recorded address.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    network_message: String,
}

#[async_trait]
impl MachineRuntime for YoukiRuntime {
    fn name(&self) -> &'static str {
        "youki"
    }

    async fn start(&self, req: &MachineRequest) -> ApiResult<RunningMachine> {
        let _guard = lock().lock().await;
        let bin = resolve_youki()?;
        // Reject an unusable state directory before reading a bundle under it.
        // A path that is not a directory otherwise surfaces as a read error.
        ensure_state_dir()?;
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
                // A rootless refusal stays on the record. Reporting the
                // request address would say the guest has a network it does not.
                let (addresses, message) = if saved.network_message.is_empty() {
                    (nic_addresses(req), String::new())
                } else {
                    (Vec::new(), saved.network_message.clone())
                };
                return Ok(RunningMachine {
                    id,
                    addresses,
                    power: PowerState::On,
                    message,
                });
            }
        }
        let mode = resolve_run_mode()?;
        // user_data and ssh_keys are not written, logged, or passed to youki.
        // PISTACK_YOUKI_NETNS skips this and is joined as the network namespace.
        let net = match provision_network(&bundle, req, &mode).await {
            Ok(net) => net,
            Err(e) => {
                if prior.is_none() {
                    let _ = remove_bundle(&bundle, &mode.privilege).await;
                }
                return Err(e);
            }
        };
        let plan = match sandbox_plan(req) {
            Ok(plan) => plan,
            Err(e) => {
                drop_link(&mode, &net).await;
                if prior.is_none() {
                    let _ = remove_bundle(&bundle, &mode.privilege).await;
                }
                return Err(e);
            }
        };
        if let Some(plan) = plan {
            let started = start_pod(&bin, &mode, &bundle, req, &plan, prior.is_some(), &net).await;
            if started.is_err() {
                drop_link(&mode, &net).await;
            }
            return started;
        }
        let host_netns = joined_netns(&net);
        if let Err(e) = write_bundle(&bundle, req, &mode, host_netns).await {
            drop_link(&mode, &net).await;
            if prior.is_none() {
                let _ = remove_bundle(&bundle, &mode.privilege).await;
            }
            return Err(e);
        }
        let id_str = id.to_string();
        let io = workload_stdio(&bundle);
        // Stop deleted the namespace. `youki start` does not join a new one,
        // so a container that has a link is created again. A first start is too.
        let create_new = prior.is_none() || joins_new_net(&net);
        if create_new {
            let wipe = if prior.is_none() {
                Some(bundle.as_path())
            } else {
                None
            };
            if let Err(e) = create_container(&bin, &mode, &bundle, &id_str, None, &io, wipe).await {
                drop_link(&mode, &net).await;
                return Err(e);
            }
            if let Err(e) = exec_detach(&bin, &mode, &["start", &id_str], None, &io).await {
                // Create succeeded and start did not. Drop the container so a
                // retry can create it again, and do not record it as running.
                // A restart keeps the bundle; the record is still on disk.
                drop_link(&mode, &net).await;
                if prior.is_none() {
                    forget_failed_create(&bin, &mode, &id_str, Some(&bundle)).await;
                } else {
                    let _ = exec_youki(&bin, &mode, &["delete", "--force", &id_str], None).await;
                }
                return Err(e);
            }
        } else if let Err(e) = exec_detach(&bin, &mode, &["start", &id_str], None, &io).await {
            drop_link(&mode, &net).await;
            return Err(e);
        }
        // youki may have loosened the bundle directory. The agent still owns
        // it unless youki changed the owner, in which case this is a no-op.
        let _ = privatize_bundle(&bundle, &mode.privilege);
        let observed = observe(req, &net);
        if let Err(e) = write_state(
            &bundle,
            &Saved {
                id: id_str,
                addresses: observed.addresses.clone(),
                power: PowerState::On,
                containers: Vec::new(),
                container_names: Vec::new(),
                probes: Vec::new(),
                netns: observed.netns.clone(),
                veths: observed.veths.clone(),
                ovs: observed.ovs.clone(),
                network_message: observed.message.clone(),
            },
        )
        .await
        {
            drop_link(&mode, &net).await;
            return Err(e);
        }
        Ok(running_from(id, &observed, PowerState::On))
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
        // Drop the veth and namespace before the kill, and before power is
        // recorded Off, so a failed release is retried.
        release_recorded(&mode, &saved, &bundle).await?;
        // Containers first, then the sandbox that owns their namespaces.
        let mut order: Vec<String> = saved.containers.iter().rev().cloned().collect();
        order.push(id.to_string());
        for cid in &order {
            exec_youki(&bin, &mode, &["kill", cid, "SIGTERM"], None).await?;
        }
        saved.power = PowerState::Off;
        saved.netns.clear();
        saved.veths.clear();
        saved.ovs.clear();
        saved.network_message.clear();
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
        if let Some(saved) = saved.as_ref() {
            release_recorded(&mode, saved, &bundle).await?;
        } else if let Some(recorded) = podnet::read_record(&bundle) {
            podnet::release(
                &ip_privilege(&mode),
                &recorded.netns,
                &recorded.veths,
                &recorded.ovs,
            )
            .await?;
        }
        let mut order: Vec<String> = saved
            .map(|s| s.containers)
            .unwrap_or_default()
            .into_iter()
            .rev()
            .collect();
        order.push(id.to_string());
        for cid in &order {
            if let Err(e) = exec_youki(&bin, &mode, &["delete", "--force", cid], None).await {
                // A bundle with no state was never created in youki. A container
                // youki has already forgotten is gone too. Anything else stays on
                // disk so a later delete can retry.
                if had_state && !container_missing(&e) {
                    return Err(e);
                }
            }
        }
        unmap_recorded(&bundle).await?;
        remove_bundle(&bundle, &mode.privilege).await
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
        let (path, rbd_mapped) = attach_host_path(req).await?;
        host_path_ok(&path)?;
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
            slot.path = path;
            slot.device = device.clone();
            slot.read_only = req.read_only;
            slot.rbd_mapped = rbd_mapped;
        } else {
            disks.push(AttachedDisk {
                volume: vol,
                path,
                device: device.clone(),
                read_only: req.read_only,
                rbd_mapped,
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
        let recorded = disks.iter().find(|d| d.volume == vol).cloned();
        release_rbd(req, recorded.as_ref()).await?;
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
                message: saved.network_message,
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

/// `$PISTACK_YOUKI_STATE`, else `$PISTACK_DATA_DIR/youki`, else
/// `/var/lib/pistack/youki`. An empty variable is treated as unset.
pub(crate) fn state_dir() -> PathBuf {
    if let Some(v) = std::env::var_os("PISTACK_YOUKI_STATE") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    if let Some(v) = std::env::var_os("PISTACK_DATA_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v).join("youki");
        }
    }
    PathBuf::from(DEFAULT_STATE)
}

fn ensure_state_dir() -> ApiResult<PathBuf> {
    let dir = state_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Err(state_dir_error(&dir, &e));
    }
    let probe = dir.join(".pistack-writable");
    match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(dir)
        }
        Err(e) => Err(state_dir_error(&dir, &e)),
    }
}

fn state_dir_error(dir: &Path, err: &std::io::Error) -> ApiError {
    ApiError::unavailable(format!(
        "youki runtime: cannot write state directory {}: {err}. Set PISTACK_YOUKI_STATE or PISTACK_DATA_DIR to a directory the agent user owns",
        dir.display()
    ))
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
const CLEANUP_ENV: &str = "PISTACK_YOUKI_CLEANUP";
const DEFAULT_CLEANUP: &str = "/usr/local/sbin/pistack-youki-cleanup";
const NETNS_ENV: &str = "PISTACK_YOUKI_NETNS";

/// Sample sudoers rule. Operators replace the user and the paths.
const SUDOERS_SAMPLE: &str =
    "ubuntu ALL=(root) NOPASSWD: /usr/bin/youki, /usr/local/sbin/pistack-youki-cleanup, /usr/sbin/ip, /usr/bin/ovs-vsctl";

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
                "youki runtime: {SUDO_ENV} is set but sudo is not available. Install sudo and a NOPASSWD sudoers rule for the youki binary, the cleanup helper, ip, and ovs-vsctl (`{SUDOERS_SAMPLE}`), or unset {SUDO_ENV} to use a rootless user namespace"
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
        "youki runtime: the agent is not root, so youki needs {SUDO_ENV}=1 with a NOPASSWD sudoers rule for the youki binary, the cleanup helper, ip, and ovs-vsctl (`{SUDOERS_SAMPLE}`) or an unprivileged user namespace for a single-id map (no newuidmap). Run the agent as root, set {SUDO_ENV}=1, or enable unprivileged user namespaces"
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

fn ip_privilege(mode: &RunMode) -> podnet::IpPrivilege {
    match &mode.privilege {
        Privilege::Root => podnet::IpPrivilege::Direct,
        Privilege::Sudo { sudo } => podnet::IpPrivilege::Sudo(sudo.clone()),
        Privilege::Rootless { .. } => podnet::IpPrivilege::Rootless,
    }
}

/// `PISTACK_YOUKI_NETNS` when it is set. Otherwise the namespace this runtime created.
fn joined_netns(net: &PodNetwork) -> Option<PathBuf> {
    if let Some(path) = requested_host_netns() {
        return Some(path);
    }
    match net {
        PodNetwork::Linked(link) => Some(link.path.clone()),
        PodNetwork::None | PodNetwork::Refused(_) => None,
    }
}

struct ObservedNet {
    addresses: Vec<String>,
    message: String,
    netns: String,
    veths: Vec<String>,
    ovs: Vec<podnet::OvsLink>,
}

fn observe(req: &MachineRequest, net: &PodNetwork) -> ObservedNet {
    match net {
        PodNetwork::Linked(link) => ObservedNet {
            addresses: nic_addresses(req),
            message: String::new(),
            netns: link.netns.clone(),
            veths: link.veths.clone(),
            ovs: link.ovs.clone(),
        },
        PodNetwork::Refused(message) => ObservedNet {
            addresses: Vec::new(),
            message: message.clone(),
            netns: String::new(),
            veths: Vec::new(),
            ovs: Vec::new(),
        },
        PodNetwork::None => ObservedNet {
            addresses: nic_addresses(req),
            message: String::new(),
            netns: String::new(),
            veths: Vec::new(),
            ovs: Vec::new(),
        },
    }
}

fn running_from(id: Id, observed: &ObservedNet, power: PowerState) -> RunningMachine {
    RunningMachine {
        id,
        addresses: observed.addresses.clone(),
        power,
        message: observed.message.clone(),
    }
}

/// Create the pod namespace unless `PISTACK_YOUKI_NETNS` already names one.
async fn provision_network(
    bundle: &Path,
    req: &MachineRequest,
    mode: &RunMode,
) -> ApiResult<PodNetwork> {
    if requested_host_netns().is_some() {
        return Ok(PodNetwork::None);
    }
    podnet::prepare(req, &ip_privilege(mode), bundle).await
}

/// Stop removed the namespace this pod was created in. The next create has
/// to run again; `youki start` keeps the namespaces from `youki create`.
fn joins_new_net(net: &PodNetwork) -> bool {
    matches!(net, PodNetwork::Linked(_))
}

async fn drop_link(mode: &RunMode, net: &PodNetwork) {
    if let PodNetwork::Linked(link) = net {
        if let Err(err) =
            podnet::release(&ip_privilege(mode), &link.netns, &link.veths, &link.ovs).await
        {
            tracing::warn!(
                netns = %link.netns,
                error = %err,
                "youki runtime: network teardown failed"
            );
        }
    }
}

/// Remove a recorded namespace. Empty names do not run `ip`.
async fn release_recorded(mode: &RunMode, saved: &Saved, bundle: &Path) -> ApiResult<()> {
    let recorded = if !saved.netns.is_empty() || !saved.veths.is_empty() || !saved.ovs.is_empty() {
        Some(podnet::RecordedNet {
            netns: saved.netns.clone(),
            veths: saved.veths.clone(),
            ovs: saved.ovs.clone(),
        })
    } else {
        podnet::read_record(bundle)
    };
    let Some(recorded) = recorded else {
        return Ok(());
    };
    podnet::release(
        &ip_privilege(mode),
        &recorded.netns,
        &recorded.veths,
        &recorded.ovs,
    )
    .await
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
    /// Slab volume id to the mounted directory recorded at attach. Loom
    /// writes this once the volume is attached. This directory is the
    /// container bind. The raw file or device in `disks.json` is not.
    #[serde(default)]
    volume_paths: BTreeMap<String, String>,
    /// Slab volume id to a device name, when the volume has no mounted
    /// directory yet. Create does not bind that device.
    #[serde(default)]
    volume_devices: BTreeMap<String, String>,
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
    volume_paths: BTreeMap<String, String>,
    volume_devices: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AttachedDisk {
    volume: String,
    path: String,
    device: String,
    #[serde(default)]
    read_only: bool,
    /// The path is a host device this agent mapped from an RBD image.
    #[serde(default, skip_serializing_if = "is_false")]
    rbd_mapped: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Host path to record, and whether detach must unmap it.
async fn attach_host_path(req: &super::VolumeAttach) -> ApiResult<(String, bool)> {
    match &req.target {
        Some(DiskTarget::Rbd {
            pool,
            image,
            monitors,
            user,
            key_secret,
        }) => {
            let path =
                super::rbdmap::map_image(pool, image, monitors, user, key_secret, req.read_only)
                    .await?;
            Ok((path, true))
        }
        _ => Ok((req.path.clone(), false)),
    }
}

async fn release_rbd(req: &super::VolumeDetach, recorded: Option<&AttachedDisk>) -> ApiResult<()> {
    match &req.target {
        Some(DiskTarget::Rbd { pool, image, .. }) => super::rbdmap::unmap_image(pool, image).await,
        _ => match recorded {
            Some(disk) if disk.rbd_mapped => super::rbdmap::unmap_device(&disk.path).await,
            _ => Ok(()),
        },
    }
}

async fn unmap_recorded(bundle: &Path) -> ApiResult<()> {
    for disk in read_disks(bundle)? {
        if disk.rbd_mapped {
            super::rbdmap::unmap_device(&disk.path).await?;
        }
    }
    Ok(())
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
    /// Mounted directories for slab volumes. Plans written before this field
    /// existed omit it; those volumes are not bound from the raw disk path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    volume_paths: BTreeMap<String, String>,
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
        volume_paths: doc.volume_paths,
        volume_devices: doc.volume_devices,
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

async fn write_bundle(
    bundle: &Path,
    req: &MachineRequest,
    mode: &RunMode,
    host_netns: Option<PathBuf>,
) -> ApiResult<()> {
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
        install_bundle(
            &bundle,
            &image,
            &hostname,
            &limits,
            launch.as_ref(),
            &mode,
            host_netns.as_deref(),
        )
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
    host_netns: Option<&Path>,
) -> ApiResult<()> {
    std::fs::create_dir_all(bundle).map_err(|e| {
        ApiError::unavailable(format!("youki runtime: creating {}: {e}", bundle.display()))
    })?;
    privatize_bundle(bundle, &mode.privilege)?;
    let rootfs = bundle.join("rootfs");
    reset_dir(&rootfs, &mode.privilege)?;
    let cfg = unpack_image_with(image, &rootfs, &mode.privilege)?;
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
    let from_env = requested_host_netns();
    let netns = from_env.as_deref().or(host_netns);
    let text = serde_json::to_vec_pretty(&mode.oci_config(
        Some(hostname),
        limits,
        (&args, &env, &cwd),
        &sandbox_namespaces(mode, netns),
        &[],
    ))
    .map_err(|e| ApiError::internal(format!("youki runtime: encoding config: {e}")))?;
    std::fs::write(bundle.join("config.json"), text)
        .map_err(|e| ApiError::unavailable(format!("youki runtime: writing config.json: {e}")))?;
    privatize_bundle(bundle, &mode.privilege)
}

fn rootfs_empty(rootfs: &Path) -> bool {
    std::fs::read_dir(rootfs)
        .map(|mut rd| rd.next().is_none())
        .unwrap_or(true)
}

fn reset_dir(dir: &Path, privilege: &Privilege) -> ApiResult<()> {
    delete_path(dir, privilege)?;
    std::fs::create_dir_all(dir).map_err(|e| {
        ApiError::unavailable(format!("youki runtime: creating {}: {e}", dir.display()))
    })
}

/// Test helper. These rootfs trees are created by the test process, so
/// removal stays unprivileged.
#[cfg(test)]
fn unpack_image(src: &Path, rootfs: &Path) -> ApiResult<ImageConfig> {
    unpack_image_with(src, rootfs, &Privilege::Root)
}

fn unpack_image_with(src: &Path, rootfs: &Path, privilege: &Privilege) -> ApiResult<ImageConfig> {
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
    unpack_archive(src, rootfs, privilege)
}

fn unpack_archive(src: &Path, rootfs: &Path, privilege: &Privilege) -> ApiResult<ImageConfig> {
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
        reset_dir(rootfs, privilege)?;
        extract_tar(&owned.path, rootfs, true)?;
        Ok(ImageConfig::default())
    }
}

struct TempPath(PathBuf);
impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        chmod_writable(&self.0);
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
    let mut dir_modes = BTreeMap::new();
    for layer in layers {
        let rel = layer.as_str().ok_or_else(cannot_unpack)?;
        apply_layer(
            &staging.join(safe_rel(rel)?),
            rootfs,
            &mut budget,
            &mut dir_modes,
        )?;
    }
    finish_dir_modes(rootfs, &dir_modes)?;
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
    let mut dir_modes = BTreeMap::new();
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
        apply_layer(
            &blob_file(layout, digest)?,
            rootfs,
            &mut budget,
            &mut dir_modes,
        )?;
    }
    finish_dir_modes(rootfs, &dir_modes)?;
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

fn apply_layer(
    path: &Path,
    rootfs: &Path,
    budget: &mut u64,
    dir_modes: &mut BTreeMap<PathBuf, u32>,
) -> ApiResult<()> {
    let owned = decompress_to_tar(path)?;
    extract_tar_budget(&owned.path, rootfs, true, budget, dir_modes)
}

fn extract_tar(path: &Path, dest: &Path, whiteouts: bool) -> ApiResult<()> {
    let mut budget = MAX_IMAGE_BYTES;
    let mut dir_modes = BTreeMap::new();
    extract_tar_budget(path, dest, whiteouts, &mut budget, &mut dir_modes)?;
    // The staging extract (whiteouts off) only classifies the archive.
    // Leave those directories writable so the temp dir can be removed.
    if whiteouts {
        finish_dir_modes(dest, &dir_modes)?;
    }
    Ok(())
}

fn extract_tar_budget(
    path: &Path,
    dest: &Path,
    whiteouts: bool,
    budget: &mut u64,
    dir_modes: &mut BTreeMap<PathBuf, u32>,
) -> ApiResult<()> {
    let anchor = Anchor::open(dest)?;
    let file = std::fs::File::open(path).map_err(|_| cannot_unpack())?;
    let mut tar = TarReader {
        inner: file,
        pending_name: None,
        pending_link: None,
    };
    let privileged = keep_setid();
    while let Some(ent) = tar.next_entry()? {
        let rel = match rel_under(&ent.name) {
            Some(rel) => rel,
            None if ent.name.is_empty() || ent.name == "." || ent.name == "./" => {
                tar.discard(ent.size)?;
                continue;
            }
            None => return Err(cannot_unpack()),
        };
        let mode = mask_mode(ent.mode, privileged);
        if whiteouts && apply_whiteout(&anchor, &rel, dir_modes)? {
            tar.discard(ent.size)?;
            continue;
        }
        match ent.flag {
            b'5' => {
                tar.discard(ent.size)?;
                anchor.ensure_dir(&rel)?;
                let key = anchor.logical_full(&rel)?;
                if key != rel {
                    dir_modes.remove(&rel);
                }
                dir_modes.insert(key, mode);
            }
            b'2' => {
                tar.discard(ent.size)?;
                forget_tree(dir_modes, &anchor, &rel);
                place_symlink_in(&anchor, &rel, &ent.link)?;
            }
            b'1' => {
                tar.discard(ent.size)?;
                forget_tree(dir_modes, &anchor, &rel);
                place_hardlink(&anchor, &rel, &ent.link)?;
            }
            b'0' | 0 => {
                forget_tree(dir_modes, &anchor, &rel);
                let dest_file = anchor.prepare_file(&rel)?;
                let mut out = open_nofollow_new(&dest_file)?;
                tar.copy_entry(ent.size, &mut out, budget)?;
                set_file_mode(&out, mode)?;
            }
            _ => tar.discard(ent.size)?,
        }
    }
    Ok(())
}

/// Setuid and setgid stay when this agent is root or sudo mode is on.
/// Rootless unpack clears them so a layer cannot plant a privileged binary.
fn keep_setid() -> bool {
    let (uid, _) = agent_ids();
    uid == 0 || env_flag(SUDO_ENV)
}

fn mask_mode(mode: u32, keep_setid: bool) -> u32 {
    let mode = mode & 0o7777;
    if keep_setid {
        mode
    } else {
        mode & !0o6000
    }
}

fn finish_dir_modes(dest: &Path, dir_modes: &BTreeMap<PathBuf, u32>) -> ApiResult<()> {
    if dir_modes.is_empty() {
        return Ok(());
    }
    let anchor = Anchor::open(dest)?;
    let mut entries: Vec<(&Path, u32)> = dir_modes.iter().map(|(p, m)| (p.as_path(), *m)).collect();
    // Deepest first, while parent directories are still searchable.
    entries.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (rel, mode) in entries {
        let Some(path) = anchor.existing(rel)? else {
            continue;
        };
        let meta = std::fs::symlink_metadata(&path).map_err(|_| cannot_unpack())?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        set_mode_nofollow(&path, mode)?;
    }
    Ok(())
}

fn forget_tree(modes: &mut BTreeMap<PathBuf, u32>, anchor: &Anchor, rel: &Path) {
    if let Ok(key) = anchor.logical_full(rel) {
        forget_path(modes, &key);
    }
    forget_path(modes, rel);
}

fn forget_path(modes: &mut BTreeMap<PathBuf, u32>, rel: &Path) {
    modes.retain(|path, _| path != rel && !path.starts_with(rel));
}

fn forget_children(modes: &mut BTreeMap<PathBuf, u32>, rel: &Path) {
    if rel.as_os_str().is_empty() {
        modes.clear();
        return;
    }
    modes.retain(|path, _| path == rel || !path.starts_with(rel));
}

/// Rootfs directory. Paths are resolved against the canonical root so a
/// symlink cannot point an extract at a host path outside it.
struct Anchor {
    root: PathBuf,
}

impl Anchor {
    fn open(dest: &Path) -> ApiResult<Self> {
        match std::fs::symlink_metadata(dest) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(cannot_unpack());
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(dest).map_err(|_| cannot_unpack())?;
            }
            Err(_) => return Err(cannot_unpack()),
        }
        let root = dest.canonicalize().map_err(|_| cannot_unpack())?;
        Ok(Self { root })
    }

    fn abs(&self, logical: &Path) -> PathBuf {
        self.root.join(logical)
    }

    /// Parent relative to the root, plus the final component.
    /// `create` makes missing parent directories writable (mode `0o755`).
    fn split(&self, rel: &Path, create: bool) -> ApiResult<Option<(PathBuf, OsString)>> {
        let mut parts = normal_parts(rel);
        let Some(name) = parts.pop() else {
            return Err(cannot_unpack());
        };
        let Some(logical) = self.walk(&parts, create)? else {
            return Ok(None);
        };
        Ok(Some((logical, name)))
    }

    fn logical_full(&self, rel: &Path) -> ApiResult<PathBuf> {
        let (logical, name) = self.split(rel, false)?.ok_or_else(cannot_unpack)?;
        Ok(logical.join(name))
    }

    fn existing(&self, rel: &Path) -> ApiResult<Option<PathBuf>> {
        let Some((logical, name)) = self.split(rel, false)? else {
            return Ok(None);
        };
        let full = self.abs(&logical).join(name);
        match std::fs::symlink_metadata(&full) {
            Ok(_) => Ok(Some(full)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(cannot_unpack()),
        }
    }

    fn prepare_file(&self, rel: &Path) -> ApiResult<PathBuf> {
        let (logical, name) = self.split(rel, true)?.ok_or_else(cannot_unpack)?;
        let dest = self.abs(&logical).join(name);
        remove_path(&dest)?;
        Ok(dest)
    }

    fn ensure_dir(&self, rel: &Path) -> ApiResult<()> {
        let (logical, name) = self.split(rel, true)?.ok_or_else(cannot_unpack)?;
        let dest = self.abs(&logical).join(name);
        match std::fs::symlink_metadata(&dest) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => loosen_dir(&dest)?,
            Ok(_) => {
                remove_path(&dest)?;
                make_writable_dir(&dest)?;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => make_writable_dir(&dest)?,
            Err(_) => return Err(cannot_unpack()),
        }
        Ok(())
    }

    /// Walk `parts`, following symlinks that stay inside the root.
    /// An absolute link, or a relative link that climbs out, is an error.
    fn walk(&self, parts: &[OsString], create: bool) -> ApiResult<Option<PathBuf>> {
        let mut logical = PathBuf::new();
        let mut pending = parts.to_vec();
        let mut index = 0usize;
        let mut hops = 0u32;
        while index < pending.len() {
            let name = pending[index].clone();
            if name.is_empty() || name == "." || name == ".." {
                return Err(cannot_unpack());
            }
            let full = self.abs(&logical).join(&name);
            match std::fs::symlink_metadata(&full) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    hops += 1;
                    if hops > 40 {
                        return Err(cannot_unpack());
                    }
                    let target = std::fs::read_link(&full).map_err(|_| cannot_unpack())?;
                    let Some(linked) = join_logical(&logical, &target) else {
                        return Err(cannot_unpack());
                    };
                    let mut next = normal_parts(&linked);
                    next.extend(pending[index + 1..].iter().cloned());
                    pending = next;
                    logical = PathBuf::new();
                    index = 0;
                }
                Ok(meta) if meta.is_dir() => {
                    logical.push(&name);
                    index += 1;
                }
                Ok(_) => return Err(cannot_unpack()),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    if !create {
                        return Ok(None);
                    }
                    make_writable_dir(&full)?;
                    logical.push(&name);
                    index += 1;
                }
                Err(_) => return Err(cannot_unpack()),
            }
        }
        Ok(Some(logical))
    }
}

fn normal_parts(path: &Path) -> Vec<OsString> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_os_string()),
            _ => None,
        })
        .collect()
}

/// `base` is relative to the root. `target` is the symlink text.
/// `None` means the link leaves the rootfs.
fn join_logical(base: &Path, target: &Path) -> Option<PathBuf> {
    if target.is_absolute() {
        return None;
    }
    let mut parts: Vec<OsString> = normal_parts(base);
    for comp in target.components() {
        match comp {
            Component::Normal(part) => {
                let text = part.to_string_lossy();
                if text.is_empty() || text == ".." || text.contains('/') {
                    return None;
                }
                parts.push(part.to_os_string());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    let mut out = PathBuf::new();
    for part in parts {
        out.push(part);
    }
    Some(out)
}

fn make_writable_dir(path: &Path) -> ApiResult<()> {
    std::fs::create_dir(path).map_err(|_| cannot_unpack())?;
    set_mode_nofollow(path, 0o755)
}

fn loosen_dir(path: &Path) -> ApiResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::symlink_metadata(path).map_err(|_| cannot_unpack())?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(cannot_unpack());
        }
        let mode = meta.permissions().mode();
        if mode & 0o700 != 0o700 {
            set_mode_nofollow(path, (mode | 0o700) & 0o7777)?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn open_nofollow_new(path: &Path) -> ApiResult<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    opts.open(path).map_err(|_| cannot_unpack())
}

fn set_file_mode(file: &std::fs::File, mode: u32) -> ApiResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = file.metadata().map_err(|_| cannot_unpack())?.permissions();
        perms.set_mode(mode & 0o7777);
        file.set_permissions(perms).map_err(|_| cannot_unpack())?;
    }
    #[cfg(not(unix))]
    {
        let _ = (file, mode);
    }
    Ok(())
}

fn set_mode_nofollow(path: &Path, mode: u32) -> ApiResult<()> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| cannot_unpack())?;
    // chmod follows a final symlink. Refuse that rather than changing a
    // target that may sit outside the rootfs. Callers pass a path whose
    // parents were already resolved inside the root.
    if meta.file_type().is_symlink() {
        return Err(cannot_unpack());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = meta.permissions();
        perms.set_mode(mode & 0o7777);
        std::fs::set_permissions(path, perms).map_err(|_| cannot_unpack())?;
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    Ok(())
}

fn apply_whiteout(
    anchor: &Anchor,
    rel: &Path,
    dir_modes: &mut BTreeMap<PathBuf, u32>,
) -> ApiResult<bool> {
    let Some(name) = rel.file_name().and_then(|s| s.to_str()) else {
        return Ok(false);
    };
    if name == ".wh..wh..opq" {
        let parent_rel = rel.parent().unwrap_or(Path::new(""));
        if parent_rel.as_os_str().is_empty() {
            clear_children(&anchor.root)?;
            dir_modes.clear();
        } else if let Some(dir) = anchor.existing(parent_rel)? {
            clear_children(&dir)?;
            if let Ok(logical) = dir.strip_prefix(&anchor.root) {
                forget_children(dir_modes, logical);
            }
            forget_children(dir_modes, parent_rel);
        }
        return Ok(true);
    }
    if let Some(rest) = name.strip_prefix(".wh.") {
        if rest.is_empty() || rest.contains('/') || rest == ".." {
            return Err(cannot_unpack());
        }
        let parent_rel = rel.parent().unwrap_or(Path::new(""));
        let target_rel = if parent_rel.as_os_str().is_empty() {
            PathBuf::from(rest)
        } else {
            parent_rel.join(rest)
        };
        if let Some(path) = anchor.existing(&target_rel)? {
            remove_path(&path)?;
        }
        if let Ok(key) = anchor.logical_full(&target_rel) {
            forget_path(dir_modes, &key);
        }
        forget_path(dir_modes, &target_rel);
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
        // A layer may have left the directory without owner write.
        chmod_writable(path);
        std::fs::remove_dir_all(path)
    };
    result.map_err(|_| cannot_unpack())
}

/// Loosen a directory this process can already access so `remove_dir_all`
/// can unlink its children. Mode 000 is left unchanged: there is no owner
/// bit to extend, and sudo mode hands that tree to the cleanup helper.
/// Symlinks are not followed. A chmod that fails (the directory is not
/// ours) is ignored.
fn chmod_deletable(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o700 != 0 && mode & 0o700 != 0o700 {
            let _ = set_mode_nofollow(path, (mode | 0o700) & 0o7777);
        }
    }
    let Ok(rd) = std::fs::read_dir(path) else {
        return;
    };
    for ent in rd.flatten() {
        chmod_deletable(&ent.path());
    }
}

/// Add owner read, write, and execute so a read-only unpacked directory can
/// still be deleted. Symlinks are not followed.
fn chmod_writable(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o700 != 0o700 {
            let _ = set_mode_nofollow(path, (mode | 0o700) & 0o7777);
        }
    }
    let Ok(rd) = std::fs::read_dir(path) else {
        return;
    };
    for ent in rd.flatten() {
        chmod_writable(&ent.path());
    }
}

fn place_symlink(root: &Path, rel: &Path, target: &str) -> ApiResult<()> {
    let anchor = Anchor::open(root)?;
    place_symlink_in(&anchor, rel, target)
}

fn place_symlink_in(anchor: &Anchor, rel: &Path, target: &str) -> ApiResult<()> {
    if !symlink_stays(rel, target) {
        return Err(cannot_unpack());
    }
    let (logical, name) = anchor.split(rel, true)?.ok_or_else(cannot_unpack)?;
    let dest = anchor.abs(&logical).join(name);
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

fn place_hardlink(anchor: &Anchor, rel: &Path, target: &str) -> ApiResult<()> {
    let target_rel = rel_under(target).ok_or_else(cannot_unpack)?;
    let Some(src) = anchor.existing(&target_rel)? else {
        return Err(cannot_unpack());
    };
    if !chain_stays(anchor, &src)? {
        return Err(cannot_unpack());
    }
    let (logical, name) = anchor.split(rel, true)?.ok_or_else(cannot_unpack)?;
    let dest = anchor.abs(&logical).join(name);
    remove_path(&dest)?;
    std::fs::hard_link(src, dest).map_err(|_| cannot_unpack())
}

/// True when `path` and every symlink it names stay inside the root.
fn chain_stays(anchor: &Anchor, path: &Path) -> ApiResult<bool> {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        let meta = std::fs::symlink_metadata(&current).map_err(|_| cannot_unpack())?;
        if !meta.file_type().is_symlink() {
            return Ok(current.starts_with(&anchor.root));
        }
        let target = std::fs::read_link(&current).map_err(|_| cannot_unpack())?;
        let Some(parent) = current.parent() else {
            return Ok(false);
        };
        let Ok(parent_rel) = parent.strip_prefix(&anchor.root) else {
            return Ok(false);
        };
        let Some(next_rel) = join_logical(parent_rel, &target) else {
            return Ok(false);
        };
        current = anchor.abs(&next_rel);
    }
    Ok(false)
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
            // After the children, same as a tar directory. A mode without
            // owner write must not block the copy that just finished.
            set_mode_nofollow(&to, source_mode(&meta))?;
        } else if meta.is_file() {
            copy_file(&from, &to, source_mode(&meta))?;
        }
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path, mode: u32) -> ApiResult<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|_| cannot_unpack())?;
    }
    let mut input = std::fs::File::open(from).map_err(|_| cannot_unpack())?;
    let mut output = std::fs::File::create(to).map_err(|_| cannot_unpack())?;
    std::io::copy(&mut input, &mut output).map_err(|_| cannot_unpack())?;
    set_file_mode(&output, mode)
}

/// Permission bits for a copied file or directory. Same mask as a tar header:
/// `0o7777`, with setuid and setgid kept only when [`keep_setid`] is set.
fn source_mode(meta: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        mask_mode(meta.permissions().mode(), keep_setid())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        0
    }
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
    mode: u32,
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
            let mode = parse_number(&header[100..108]).ok_or_else(cannot_unpack)?;
            let mode = u32::try_from(mode & 0o7777).unwrap_or(0);
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
                mode,
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

async fn remove_bundle(bundle: &Path, privilege: &Privilege) -> ApiResult<()> {
    let bundle = bundle.to_path_buf();
    let privilege = privilege.clone();
    match tokio::task::spawn_blocking(move || delete_bundle(&bundle, &privilege)).await {
        Ok(result) => result,
        Err(_) => Err(ApiError::unavailable(
            "youki runtime: bundle removal stopped",
        )),
    }
}

/// Unlink a symlink `rootfs` without following it, then remove the bundle.
fn delete_bundle(bundle: &Path, privilege: &Privilege) -> ApiResult<()> {
    let rootfs = bundle.join("rootfs");
    if let Ok(meta) = std::fs::symlink_metadata(&rootfs) {
        if meta.file_type().is_symlink() {
            delete_path(&rootfs, privilege)?;
        }
    }
    delete_path(bundle, privilege)
}

fn delete_path(path: &Path, privilege: &Privilege) -> ApiResult<()> {
    match remove_unprivileged(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) if permission_denied(&e) && matches!(privilege, Privilege::Sudo { .. }) => {
            privileged_remove(path, privilege, &e)
        }
        Err(e) => Err(remove_err(path, &e)),
    }
}

fn remove_unprivileged(path: &Path) -> std::io::Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        std::fs::remove_file(path)
    } else {
        // A layer directory such as 0555 is still ours to delete when this
        // uid can already access it. Mode 000 stays locked, and a directory
        // we do not own is unchanged when chmod fails. Sudo mode removes
        // those through the cleanup helper.
        chmod_deletable(path);
        std::fs::remove_dir_all(path)
    }
}

fn permission_denied(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::PermissionDenied
}

fn remove_err(path: &Path, err: &std::io::Error) -> ApiError {
    ApiError::unavailable(format!("youki runtime: removing {}: {err}", path.display()))
}

/// `sudo -n <helper> delete --path <path>`. The path is checked here, and
/// the helper checks it again.
fn privileged_remove(path: &Path, privilege: &Privilege, cause: &std::io::Error) -> ApiResult<()> {
    let Privilege::Sudo { sudo } = privilege else {
        return Err(remove_err(path, cause));
    };
    let path = match checked_removal_path(path) {
        Ok(path) => path,
        Err(()) => {
            return Err(ApiError::unavailable(format!(
                "youki runtime: removing {}: {cause}. Refusing privileged removal of a path outside the youki state directory or containing '..'",
                path.display()
            )));
        }
    };
    let Some(helper) = cleanup_helper() else {
        return Err(ApiError::unavailable(format!(
            "youki runtime: removing {}: {cause}. Install {DEFAULT_CLEANUP} and allow it in sudoers (`{SUDOERS_SAMPLE}`), or set {CLEANUP_ENV}",
            path.display()
        )));
    };
    let output = std::process::Command::new(sudo)
        .arg("-n")
        .arg(&helper)
        .arg("delete")
        .arg("--path")
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| {
            ApiError::unavailable(format!(
                "youki runtime: cannot run cleanup helper {}: {e}",
                helper.display()
            ))
        })?;
    if output.status.success() && !path_exists(&path) {
        return Ok(());
    }
    if output.status.success() {
        return Err(ApiError::unavailable(format!(
            "youki runtime: cleanup helper left {} in place",
            path.display()
        )));
    }
    Err(ApiError::unavailable(format!(
        "youki runtime: removing {} failed: {}",
        path.display(),
        stderr_tail(&output.stderr, output.status)
    )))
}

fn path_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Absolute path strictly inside [`state_dir`], with no `..` component.
/// Lexical only: a symlink is not resolved, so the check cannot be pointed
/// at another directory by canonicalizing it.
fn checked_removal_path(path: &Path) -> Result<PathBuf, ()> {
    if !removal_text_ok(path) || !path.is_absolute() {
        return Err(());
    }
    let state = state_dir();
    if !removal_text_ok(&state) || !state.is_absolute() {
        return Err(());
    }
    let path = normalize_lexical(path);
    let state = normalize_lexical(&state);
    if path == state || !path.starts_with(&state) {
        return Err(());
    }
    Ok(path)
}

fn removal_text_ok(path: &Path) -> bool {
    let text = path.to_string_lossy();
    if text.contains('\0') || text.chars().any(|ch| ch.is_control() || ch == '\u{7f}') {
        return false;
    }
    !text.split('/').any(|part| part == "..")
        && !path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => out.push(".."),
            Component::RootDir => out.push("/"),
            Component::Normal(part) => out.push(part),
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
        }
    }
    out
}

/// `PISTACK_YOUKI_CLEANUP` replaces the installed helper. An empty value is ignored.
fn cleanup_helper() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(CLEANUP_ENV) {
        if !value.is_empty() {
            let path = PathBuf::from(value);
            return is_executable(&path).then_some(path);
        }
    }
    let path = PathBuf::from(DEFAULT_CLEANUP);
    is_executable(&path).then_some(path)
}

/// In sudo mode the bundle is `0700` and owned by the agent. Other host
/// users cannot traverse it. Root and rootless leave the mode alone.
fn privatize_bundle(bundle: &Path, privilege: &Privilege) -> ApiResult<()> {
    if matches!(privilege, Privilege::Sudo { .. }) {
        set_mode(bundle, 0o700)?;
    }
    Ok(())
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
        let disks = read_disks(bundle)?;
        ensure_slab_sources(plan, &disks)?;
        let processes = prepare_pod(bundle, hostname, limits, plan, fallback_image, self, None)?;
        write_container_configs(bundle, plan, &processes, &disks, limits, pid, self)
    }
}

fn prepare_pod(
    bundle: &Path,
    hostname: &str,
    limits: &Limits,
    plan: &PodPlan,
    fallback_image: &str,
    mode: &RunMode,
    host_netns: Option<&Path>,
) -> ApiResult<Vec<PreparedProcess>> {
    std::fs::create_dir_all(bundle).map_err(|e| volume_io(bundle, e))?;
    privatize_bundle(bundle, &mode.privilege)?;
    materialize_volumes(bundle, plan, &mode.privilege)?;
    write_mount_plan(bundle, plan)?;
    write_pause_rootfs(bundle, hostname, limits, mode, host_netns)?;
    let mut processes = Vec::new();
    for (index, container) in plan.containers.iter().enumerate() {
        let image = choose_container_image(index, &container.image, fallback_image)
            .map_err(|_| cannot_unpack_container(container, index))?;
        let dir = container_dir(bundle, index);
        std::fs::create_dir_all(&dir).map_err(|e| volume_io(&dir, e))?;
        let rootfs = dir.join("rootfs");
        reset_dir(&rootfs, &mode.privilege)?;
        let cfg = unpack_image_with(&image, &rootfs, &mode.privilege)?;
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
    privatize_bundle(bundle, &mode.privilege)?;
    Ok(processes)
}

fn write_pause_rootfs(
    bundle: &Path,
    hostname: &str,
    limits: &Limits,
    mode: &RunMode,
    host_netns: Option<&Path>,
) -> ApiResult<()> {
    let rootfs = bundle.join("rootfs");
    reset_dir(&rootfs, &mode.privilege)?;
    write_mode(&rootfs.join("pause"), PAUSE_ELF, 0o755)?;
    let args = vec!["/pause".to_string()];
    let env = vec![format!("PATH={DEFAULT_PATH}")];
    let from_env = requested_host_netns();
    let netns = from_env.as_deref().or(host_netns);
    let spec = mode.oci_config(
        Some(hostname),
        limits,
        (&args, &env, "/"),
        &sandbox_namespaces(mode, netns),
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
    materialize_volumes(bundle, plan, &mode.privilege)?;
    let rootless = matches!(&mode.privilege, Privilege::Rootless { .. });
    let namespaces = container_namespaces(pid, rootless);
    for (index, process) in processes.iter().enumerate() {
        let mounts = plan
            .containers
            .get(index)
            .map(|container| container.volume_mounts.as_slice())
            .unwrap_or(&[]);
        let binds = pod_binds(bundle, plan, mounts, disks)?;
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
        volume_paths: plan.volume_paths.clone(),
    };
    write_json(&pod.join("mount-plan.json"), &planned, 0o644)
}

/// ConfigMap and Secret directories are replaced. EmptyDir is created once
/// and kept. Slab volumes are not files on the pod directory.
fn materialize_volumes(pod: &Path, plan: &PodPlan, privilege: &Privilege) -> ApiResult<()> {
    for (index, vol) in plan.volumes.iter().enumerate() {
        match &vol.source {
            VolumeSrc::EmptyDir => {
                let dir = volume_host_dir(pod, index, &vol.name);
                std::fs::create_dir_all(&dir).map_err(|e| volume_io(&dir, e))?;
            }
            VolumeSrc::ConfigMap(name) => {
                let dir = volume_host_dir(pod, index, &vol.name);
                let (file_mode, dir_mode) = projected_modes(privilege, false);
                match plan.config_maps.get(name) {
                    Some(files) => write_projected(&dir, files, file_mode, dir_mode, privilege)?,
                    None => {
                        reset_dir(&dir, privilege)?;
                        if matches!(privilege, Privilege::Sudo { .. }) {
                            set_mode(&dir, dir_mode)?;
                        }
                    }
                }
            }
            VolumeSrc::Secret(name) => {
                let dir = volume_host_dir(pod, index, &vol.name);
                let (file_mode, dir_mode) = projected_modes(privilege, true);
                match plan.secrets.get(name) {
                    Some(files) => write_projected(&dir, files, file_mode, dir_mode, privilege)?,
                    None => {
                        reset_dir(&dir, privilege)?;
                        set_mode(&dir, dir_mode)?;
                    }
                }
            }
            VolumeSrc::Volume(_) => {}
        }
    }
    Ok(())
}

/// `(file mode, directory mode)`. Sudo mode opens Secret files to the
/// container user. Root and rootless keep them private to the owner.
fn projected_modes(privilege: &Privilege, secret: bool) -> (u32, u32) {
    if matches!(privilege, Privilege::Sudo { .. }) {
        // 0644, not 0640. The container user is uid 0 / gid 0, not the
        // agent's group, and may not have CAP_DAC_OVERRIDE. The bundle
        // directory is 0700, so other host users cannot traverse here.
        return (0o644, 0o755);
    }
    if secret {
        (0o400, 0o700)
    } else {
        (0o644, 0o755)
    }
}

fn write_projected(
    dir: &Path,
    files: &Projected,
    file_mode: u32,
    dir_mode: u32,
    privilege: &Privilege,
) -> ApiResult<()> {
    reset_dir(dir, privilege)?;
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
    slab_paths: &BTreeMap<String, String>,
) -> Option<PreparedVolume> {
    match &vol.source {
        VolumeSrc::EmptyDir | VolumeSrc::ConfigMap(_) | VolumeSrc::Secret(_) => {
            Some(PreparedVolume {
                host_path: volume_host_dir(pod, index, &vol.name),
                directory: true,
                force_read_only: false,
            })
        }
        // The bind is the mounted folder. A raw file or device in
        // `disks.json` is not a source. Attach rewrites this config before
        // the folder is recorded, so a missing path, a non-directory, or a
        // directory that is not a mount point is left unbound instead of
        // failing the attach.
        VolumeSrc::Volume(id) => {
            let disk = disks.iter().find(|d| &d.volume == id)?;
            let path = slab_paths.get(id).filter(|path| !path.is_empty())?;
            let host_path = PathBuf::from(path);
            if !host_path.is_dir() || !matches!(folder_is_mount_point(path), Ok(true)) {
                return None;
            }
            Some(PreparedVolume {
                host_path,
                directory: true,
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
    slab_paths: &BTreeMap<String, String>,
) -> Vec<OciBind> {
    let mut out = Vec::new();
    for mount in mounts {
        if mount.name.is_empty() || !valid_destination(&mount.mount_path) {
            continue;
        }
        let Some(index) = volumes.iter().position(|v| v.name == mount.name) else {
            continue;
        };
        let Some(prepared) = prepared_volume(pod, index, &volumes[index], disks, slab_paths) else {
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

/// Binds for one container at create time. A slab volume fails when its
/// folder is missing, is not a directory, or is not a mount point. Other
/// volumes keep the lenient mount plan.
fn pod_binds(
    pod: &Path,
    plan: &PodPlan,
    mounts: &[SandboxMount],
    disks: &[AttachedDisk],
) -> ApiResult<Vec<OciBind>> {
    let mut out = Vec::new();
    for mount in mounts {
        if mount.name.is_empty() || !valid_destination(&mount.mount_path) {
            continue;
        }
        let Some(index) = plan.volumes.iter().position(|v| v.name == mount.name) else {
            continue;
        };
        let vol = &plan.volumes[index];
        let prepared = match &vol.source {
            VolumeSrc::Volume(id) => match slab_prepared(vol, id, plan, disks)? {
                Some(prepared) => prepared,
                None => continue,
            },
            _ => match prepared_volume(pod, index, vol, disks, &plan.volume_paths) {
                Some(prepared) => prepared,
                None => continue,
            },
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
    Ok(out)
}

/// Every mounted slab volume resolves, or create stops.
fn ensure_slab_sources(plan: &PodPlan, disks: &[AttachedDisk]) -> ApiResult<()> {
    for vol in &plan.volumes {
        let VolumeSrc::Volume(id) = &vol.source else {
            continue;
        };
        let used = plan.containers.iter().any(|container| {
            container.volume_mounts.iter().any(|mount| {
                mount.name == vol.name
                    && !mount.name.is_empty()
                    && valid_destination(&mount.mount_path)
            })
        });
        if used {
            slab_prepared(vol, id, plan, disks)?;
        }
    }
    Ok(())
}

fn slab_prepared(
    vol: &SandboxVolume,
    id: &str,
    plan: &PodPlan,
    disks: &[AttachedDisk],
) -> ApiResult<Option<PreparedVolume>> {
    let disk = disks.iter().find(|d| d.volume == id);
    let label = volume_label(vol, id);
    // The recorded host path is the mounted folder. `disks.json` keeps the
    // backing file or mapped device for detach and is not a bind source.
    if let Some(path) = plan
        .volume_paths
        .get(id)
        .map(String::as_str)
        .filter(|path| !path.is_empty())
    {
        let host_path = require_mounted_dir(label, path)?;
        return Ok(Some(PreparedVolume {
            host_path,
            directory: true,
            force_read_only: disk.is_some_and(|d| d.read_only),
        }));
    }
    if let Some(disk) = disk.filter(|d| !d.path.is_empty()) {
        return Err(raw_source_error(label, &disk.path));
    }
    // A device name and no mounted directory yet. Mapping may store the
    // device after the bundle exists; create does not bind it.
    if plan.volume_devices.contains_key(id) {
        return Ok(None);
    }
    Err(ApiError::unavailable(format!(
        "youki runtime: slab volume {label} has no host path"
    )))
}

fn volume_label<'a>(vol: &'a SandboxVolume, id: &'a str) -> &'a str {
    if vol.name.is_empty() {
        id
    } else {
        vol.name.as_str()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlabSource {
    Missing,
    Dir,
    File,
    Block,
    Other,
}

/// The mounted folder, or an error that names the volume.
fn require_mounted_dir(label: &str, path: &str) -> ApiResult<PathBuf> {
    match classify_slab_path(path) {
        Ok(SlabSource::Dir) => match folder_is_mount_point(path) {
            Ok(true) => Ok(PathBuf::from(path)),
            Ok(false) => Err(not_mounted_error(label, path)),
            Err(err) => Err(slab_source_io(label, path, err)),
        },
        Ok(kind) => Err(slab_source_error(label, path, kind)),
        Err(err) => Err(slab_source_io(label, path, err)),
    }
}

fn not_mounted_error(label: &str, path: &str) -> ApiError {
    ApiError::unavailable(format!(
        "youki runtime: slab volume {label} is not mounted at {path}"
    ))
}

/// `$PISTACK_YOUKI_MOUNTINFO` replaces `/proc/self/mountinfo` for tests.
/// An empty value is ignored.
const MOUNTINFO_ENV: &str = "PISTACK_YOUKI_MOUNTINFO";

fn mountinfo_file() -> PathBuf {
    match std::env::var_os(MOUNTINFO_ENV) {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from("/proc/self/mountinfo"),
    }
}

/// Whether `path` is a mount point.
///
/// The mount table is preferred. The path has to appear as a mount point,
/// after octal escapes such as `\040` are turned back into characters. A
/// bind on the same disk does not change the device number, so the table
/// is what catches that. When the table cannot be read, the folder's
/// device number is compared with its parent's.
fn folder_is_mount_point(path: &str) -> std::io::Result<bool> {
    match std::fs::read_to_string(mountinfo_file()) {
        Ok(text) => Ok(mountinfo_lists(&text, path)),
        Err(_) => mounted_by_device(Path::new(path)),
    }
}

fn mountinfo_lists(text: &str, path: &str) -> bool {
    let want = mount_key(path);
    text.lines()
        .any(|line| mountinfo_point(line).is_some_and(|point| mount_key(&point) == want))
}

/// Field 5 of a mountinfo line is the mount point. Optional fields sit
/// after the mount options, so the point stays the fifth word.
fn mountinfo_point(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    line.split_whitespace().nth(4).map(unescape_mount)
}

fn mount_key(path: &str) -> &str {
    if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    }
}

/// Mountinfo escapes space, tab, newline, and backslash as three octal digits.
fn unescape_mount(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 4 <= bytes.len() {
            let oct = &bytes[i + 1..i + 4];
            if oct.iter().all(|b| (b'0'..=b'7').contains(b)) {
                if let Ok(text) = std::str::from_utf8(oct) {
                    if let Ok(v) = u8::from_str_radix(text, 8) {
                        out.push(v);
                        i += 4;
                        continue;
                    }
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn mounted_by_device(path: &Path) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let child = std::fs::metadata(path)?;
        let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        else {
            return Ok(true);
        };
        let parent = std::fs::metadata(parent)?;
        Ok(child.dev() != parent.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "mount check needs a unix device number",
        ))
    }
}

/// `disks.json` recorded a backing file or device. That is not the bind.
fn raw_source_error(label: &str, path: &str) -> ApiError {
    match classify_slab_path(path) {
        Ok(SlabSource::Dir) | Ok(SlabSource::Other) => {
            slab_source_error(label, path, SlabSource::Other)
        }
        Ok(kind) => slab_source_error(label, path, kind),
        Err(err) => slab_source_io(label, path, err),
    }
}

fn slab_source_error(label: &str, path: &str, kind: SlabSource) -> ApiError {
    let why = match kind {
        SlabSource::Missing => "does not exist",
        SlabSource::File => "is a file, not a mounted directory",
        SlabSource::Block => "is a block device, not a mounted directory",
        SlabSource::Dir | SlabSource::Other => "is not a mounted directory",
    };
    ApiError::unavailable(format!(
        "youki runtime: slab volume {label} source {path} {why}"
    ))
}

fn slab_source_io(label: &str, path: &str, err: std::io::Error) -> ApiError {
    ApiError::unavailable(format!(
        "youki runtime: slab volume {label} source {path}: {err}"
    ))
}

fn classify_slab_path(path: &str) -> std::io::Result<SlabSource> {
    let p = Path::new(path);
    if path.is_empty() || path.contains('\0') || !p.is_absolute() || has_parent_dir(p) {
        return Ok(SlabSource::Other);
    }
    match std::fs::metadata(p) {
        Ok(meta) if meta.is_dir() => Ok(SlabSource::Dir),
        Ok(meta) if is_block_device(&meta) => Ok(SlabSource::Block),
        Ok(meta) if meta.is_file() => Ok(SlabSource::File),
        Ok(_) => Ok(SlabSource::Other),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(SlabSource::Missing),
        Err(err) => Err(err),
    }
}

fn is_block_device(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        meta.file_type().is_block_device()
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
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
        let binds = oci_binds(
            bundle,
            &plan.volumes,
            &container.mounts,
            &disks,
            &plan.volume_paths,
        );
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
    net: &PodNetwork,
) -> ApiResult<RunningMachine> {
    // Fail before youki create. A missing path, or a directory that is not
    // a mount point, would otherwise land in config.json and the pod would
    // write onto the host disk.
    let disks = read_disks(bundle)?;
    ensure_slab_sources(plan, &disks)?;
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
    let observed = observe(req, net);
    let bundle_prep = bundle.to_path_buf();
    let plan_prep = plan.clone();
    let host_prep = hostname;
    let image = req.image.clone();
    let mode_prep = mode.clone();
    let host_netns = joined_netns(net);
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_pod(
            &bundle_prep,
            &host_prep,
            &limits,
            &plan_prep,
            &image,
            &mode_prep,
            host_netns.as_deref(),
        )
    })
    .await
    .map_err(|_| ApiError::unavailable("youki runtime: image unpack stopped"))?;
    let processes = match prepared {
        Ok(processes) => processes,
        Err(e) => {
            if !prior {
                let _ = remove_bundle(bundle, &mode.privilege).await;
            }
            return Err(e);
        }
    };
    let id_str = id.to_string();
    let pause = pause_stdio(bundle);
    // A stopped pod's namespace is gone. Create the sandbox again so it
    // joins the one `provision_network` just made. `youki start` alone
    // would keep the empty namespace from the first create.
    let recreate = joins_new_net(net);
    if !prior || recreate {
        let wipe = if prior { None } else { Some(bundle) };
        create_container(bin, mode, bundle, &id_str, None, &pause, wipe).await?;
        if let Err(e) = exec_detach(bin, mode, &["start", &id_str], None, &pause).await {
            release_failed_create(bin, mode, bundle, &id_str, &[], prior).await;
            return Err(e);
        }
    } else {
        exec_detach(bin, mode, &["start", &id_str], None, &pause).await?;
    }
    let state = match run_youki(bin, mode, &["state", &id_str], None).await {
        Ok(output) => output,
        Err(e) => {
            if !prior || recreate {
                release_failed_create(bin, mode, bundle, &id_str, &[], prior).await;
            }
            return Err(e);
        }
    };
    let Some(pid) = parse_state_pid(&state.stdout) else {
        if !prior || recreate {
            release_failed_create(bin, mode, bundle, &id_str, &[], prior).await;
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
        if !prior || recreate {
            release_failed_create(bin, mode, bundle, &id_str, &[], prior).await;
        }
        return Err(e);
    }
    let user_ns = user_ns_join(mode, pid);
    let mut started = Vec::new();
    for index in 0..plan.containers.len() {
        let cid = container_youki_id(&id, index);
        let cpath = container_dir(bundle, index);
        let io = workload_stdio(&cpath);
        if let Err(e) =
            create_container(bin, mode, &cpath, &cid, user_ns.as_deref(), &io, None).await
        {
            if !prior || recreate {
                release_failed_create(bin, mode, bundle, &id_str, &started, prior).await;
            }
            return Err(e);
        }
        if let Err(e) = exec_detach(bin, mode, &["start", &cid], user_ns.as_deref(), &io).await {
            if !prior || recreate {
                let _ = exec_youki(bin, mode, &["delete", "--force", &cid], None).await;
                release_failed_create(bin, mode, bundle, &id_str, &started, prior).await;
            }
            return Err(e);
        }
        started.push(cid);
    }
    let _ = privatize_bundle(bundle, &mode.privilege);
    write_state(
        bundle,
        &Saved {
            id: id_str,
            addresses: observed.addresses.clone(),
            power: PowerState::On,
            containers: started,
            container_names: plan.containers.iter().map(|c| c.name.clone()).collect(),
            probes: crate::probe::probes_in_user_data(req.user_data.as_deref()),
            netns: observed.netns.clone(),
            veths: observed.veths.clone(),
            ovs: observed.ovs.clone(),
            network_message: observed.message.clone(),
        },
    )
    .await?;
    Ok(running_from(id, &observed, PowerState::On))
}

async fn abort_new_pod(
    bin: &Path,
    mode: &RunMode,
    bundle: &Path,
    sandbox_id: &str,
    created: &[String],
) {
    for cid in created.iter().rev() {
        let _ = exec_youki(bin, mode, &["delete", "--force", cid], None).await;
    }
    let _ = exec_youki(bin, mode, &["delete", "--force", sandbox_id], None).await;
    let _ = remove_bundle(bundle, &mode.privilege).await;
}

/// Drop containers from a create that did not finish. A first create also
/// removes the bundle. A restart keeps it: the volume records still live there.
async fn release_failed_create(
    bin: &Path,
    mode: &RunMode,
    bundle: &Path,
    sandbox_id: &str,
    created: &[String],
    prior: bool,
) {
    if prior {
        for cid in created.iter().rev() {
            let _ = exec_youki(bin, mode, &["delete", "--force", cid], None).await;
        }
        let _ = exec_youki(bin, mode, &["delete", "--force", sandbox_id], None).await;
        return;
    }
    abort_new_pod(bin, mode, bundle, sandbox_id, created).await;
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

/// Stdio for `create` and `start`. The container init inherits these fds, so
/// none of them is a pipe. `None` is `/dev/null` (the sandbox pause process).
struct DetachIo {
    stdout: Option<PathBuf>,
    stderr: Option<PathBuf>,
    /// Agent-created file passed as `youki --log`. Read after youki exits.
    youki_log: PathBuf,
}

fn workload_stdio(bundle: &Path) -> DetachIo {
    let log = bundle.join("log");
    DetachIo {
        stdout: Some(log.clone()),
        stderr: Some(log),
        youki_log: bundle.join("youki.log"),
    }
}

fn pause_stdio(bundle: &Path) -> DetachIo {
    DetachIo {
        stdout: None,
        stderr: None,
        youki_log: bundle.join("youki.log"),
    }
}

/// `youki state` shows a container this agent has no `state.json` record of
/// as running. Remove it so `create` does not fail with "already exists".
async fn clear_leftover(bin: &Path, mode: &RunMode, id: &str) -> ApiResult<()> {
    match run_youki(bin, mode, &["state", id], None).await {
        Ok(_) => exec_youki(bin, mode, &["delete", "--force", id], None).await,
        Err(e) if container_missing(&e) => Ok(()),
        Err(e) => Err(e),
    }
}

async fn forget_failed_create(bin: &Path, mode: &RunMode, id: &str, bundle: Option<&Path>) {
    let _ = exec_youki(bin, mode, &["delete", "--force", id], None).await;
    if let Some(bundle) = bundle {
        let _ = remove_bundle(bundle, &mode.privilege).await;
    }
}

async fn create_container(
    bin: &Path,
    mode: &RunMode,
    bundle: &Path,
    id: &str,
    join_user_ns: Option<&Path>,
    io: &DetachIo,
    drop_bundle: Option<&Path>,
) -> ApiResult<()> {
    if let Err(e) = clear_leftover(bin, mode, id).await {
        if let Some(root) = drop_bundle {
            let _ = remove_bundle(root, &mode.privilege).await;
        }
        return Err(e);
    }
    let bundle_arg = path_arg(bundle);
    if let Err(e) = exec_detach(
        bin,
        mode,
        &["create", "--bundle", &bundle_arg, id],
        join_user_ns,
        io,
    )
    .await
    {
        forget_failed_create(bin, mode, id, drop_bundle).await;
        return Err(e);
    }
    Ok(())
}

async fn exec_detach(
    bin: &Path,
    mode: &RunMode,
    args: &[&str],
    join_user_ns: Option<&Path>,
    io: &DetachIo,
) -> ApiResult<()> {
    run_detach(bin, mode, args, join_user_ns, io)
        .await
        .map(|_| ())
}

/// `create` / `start`. Stdout and stderr are files or `/dev/null`, opened
/// here so a sudo'd youki still writes through the agent's credentials.
/// youki's own errors are `--log` plus that stderr file, read after exit.
async fn run_detach(
    bin: &Path,
    mode: &RunMode,
    args: &[&str],
    join_user_ns: Option<&Path>,
    io: &DetachIo,
) -> ApiResult<std::process::Output> {
    // Create the log as the agent. Root (sudo) then appends to an existing
    // file instead of making a root-owned one the agent cannot read.
    let _created = open_append(&io.youki_log)?;
    let mut cmd_args = Vec::with_capacity(args.len() + 2);
    cmd_args.push("--log".to_string());
    cmd_args.push(path_arg(&io.youki_log));
    cmd_args.extend(args.iter().copied().map(str::to_string));

    let mut cmd = youki_command(bin, &mode.privilege);
    cmd.args(&cmd_args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(stdio_target(io.stdout.as_deref())?)
        .stderr(stdio_target(io.stderr.as_deref())?);
    #[cfg(unix)]
    cmd.process_group(0);
    arm_user_ns_join(&mut cmd, join_user_ns);
    let program = command_program(bin, mode);
    let mut child = cmd
        .spawn()
        .map_err(|e| ApiError::unavailable(format!("youki runtime: cannot run {program}: {e}")))?;
    let mut group = GroupKill::arm(child.id());
    let timeout = youki_timeout();
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            group.disarm();
            status
        }
        Ok(Err(e)) => {
            drop(group);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            return Err(ApiError::unavailable(format!(
                "youki runtime: {}: {e}",
                bin.display()
            )));
        }
        Err(_) => {
            drop(group);
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            let op = args.first().copied().unwrap_or("youki");
            return Err(ApiError::unavailable(format!(
                "youki runtime: {op} timed out after {}",
                timeout_label(timeout)
            )));
        }
    };
    let output = std::process::Output {
        status,
        stdout: Vec::new(),
        stderr: detach_stderr(io),
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

fn detach_stderr(io: &DetachIo) -> Vec<u8> {
    if let Some(path) = io.stderr.as_deref() {
        if let Ok(bytes) = std::fs::read(path) {
            if !bytes.is_empty() {
                return bytes;
            }
        }
    }
    std::fs::read(&io.youki_log).unwrap_or_default()
}

fn stdio_target(path: Option<&Path>) -> ApiResult<Stdio> {
    match path {
        None => Ok(Stdio::null()),
        Some(path) => Ok(Stdio::from(open_append(path)?)),
    }
}

fn open_append(path: &Path) -> ApiResult<std::fs::File> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ApiError::unavailable(format!("youki runtime: creating {}: {e}", parent.display()))
            })?;
        }
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| {
            ApiError::unavailable(format!("youki runtime: opening {}: {e}", path.display()))
        })
}

fn command_program(bin: &Path, mode: &RunMode) -> String {
    match &mode.privilege {
        Privilege::Sudo { sudo } => sudo.display().to_string(),
        Privilege::Root | Privilege::Rootless { .. } => bin.display().to_string(),
    }
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
    let program = command_program(bin, mode);
    let child = cmd
        .spawn()
        .map_err(|e| ApiError::unavailable(format!("youki runtime: cannot run {program}: {e}")))?;
    let timeout = youki_timeout();
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return Err(ApiError::unavailable(format!(
                "youki runtime: {}: {e}",
                bin.display()
            )));
        }
        Err(_) => {
            let op = args.first().copied().unwrap_or("youki");
            return Err(ApiError::unavailable(format!(
                "youki runtime: {op} timed out after {}",
                timeout_label(timeout)
            )));
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
                TIMEOUT_ENV,
                SUDO_ENV,
                SUDO_BIN_ENV,
                CLEANUP_ENV,
                NETNS_ENV,
                "PISTACK_YOUKI_TEST_EUID",
                "PISTACK_YOUKI_TEST_EGID",
                "PISTACK_YOUKI_TEST_USERNS",
                "PISTACK_RBD",
                "PISTACK_RBD_SUDO",
                "PISTACK_RBD_SUDO_BIN",
                "PISTACK_RBD_TEST_EUID",
                "PISTACK_RBD_DEV_ROOT",
                "PISTACK_IP_BIN",
            ];
            let saved = keys
                .into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect();
            std::env::set_var("PISTACK_YOUKI_BIN", bin);
            std::env::set_var("PISTACK_YOUKI_STATE", state);
            std::env::set_var("PISTACK_YOUKI_TEST_EUID", "0");
            for key in [
                TIMEOUT_ENV,
                SUDO_ENV,
                SUDO_BIN_ENV,
                CLEANUP_ENV,
                NETNS_ENV,
                "PISTACK_YOUKI_TEST_EGID",
                "PISTACK_YOUKI_TEST_USERNS",
                "PISTACK_RBD",
                "PISTACK_RBD_SUDO",
                "PISTACK_RBD_SUDO_BIN",
                "PISTACK_RBD_TEST_EUID",
                "PISTACK_RBD_DEV_ROOT",
                "PISTACK_IP_BIN",
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

        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvSlot {
        fn drop(&mut self) {
            restore(self.key, self.prev.take());
        }
    }

    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    fn write_exe(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    /// Fake youki. `state` reports a container only after `create` (or when
    /// the test pre-seeds `youki-ids/<id>`). Missing containers say
    /// "does not exist" so a pre-create check does not delete them.
    fn write_script(path: &Path, log: &Path, exit_code: i32) {
        let quoted = shell_quote(log);
        let track = shell_quote(&log.parent().unwrap().join("youki-ids"));
        let stderr = if exit_code == 0 {
            String::new()
        } else {
            "echo 'youki exploded' >&2\n".to_owned()
        };
        let body = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" >> {quoted}
printf '\n' >> {quoted}
mkdir -p {track}
cmd=
id=
skip=0
for arg in "$@"; do
  if [ "$skip" = 1 ]; then
    skip=0
    continue
  fi
  case "$arg" in
    --log|--bundle|--cwd|--env)
      skip=1
      ;;
    --*)
      ;;
    *)
      if [ -z "$cmd" ]; then
        cmd=$arg
      fi
      id=$arg
      ;;
  esac
done
if [ "$cmd" = state ]; then
  if [ -f {track}/"$id" ]; then
    printf '%s\n' '{{"pid":4242,"status":"running"}}'
    exit 0
  fi
  echo "container $id does not exist" >&2
  exit 1
fi
if [ "$cmd" = create ]; then
  : > {track}/"$id"
fi
if [ "$cmd" = delete ]; then
  rm -f {track}/"$id"
fi
{stderr}exit {exit_code}
"#
        );
        write_exe(path, &body);
    }

    fn req(name: &str, image: &str, address: &str) -> MachineRequest {
        MachineRequest {
            id: Id::new(kind::MACHINE),
            name: name.into(),
            kind: MachineKind::Container,
            image: image.into(),
            image_source: None,
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

    fn mode_bits(path: &Path) -> u32 {
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
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
        tar_header_full(name, size, flag, 0o644, "")
    }

    fn tar_header_full(name: &str, size: u64, flag: u8, mode: u32, link: &str) -> [u8; 512] {
        let mut header = [0u8; 512];
        assert!(name.len() < 100, "{name}");
        assert!(link.len() < 100, "{link}");
        header[..name.len()].copy_from_slice(name.as_bytes());
        if !link.is_empty() {
            header[157..157 + link.len()].copy_from_slice(link.as_bytes());
        }
        put_octal(&mut header[100..108], u64::from(mode));
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

    fn tar_nodes(entries: &[(&str, &[u8], u8, u32, &str)]) -> Vec<u8> {
        let mut buf = Vec::new();
        for (name, data, flag, mode, link) in entries {
            buf.extend_from_slice(&tar_header_full(
                name,
                data.len() as u64,
                *flag,
                *mode,
                link,
            ));
            buf.extend_from_slice(data);
            let pad = (512 - (data.len() % 512)) % 512;
            buf.extend(std::iter::repeat_n(0u8, pad));
        }
        buf.extend(std::iter::repeat_n(0u8, 1024));
        buf
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
        // One state per create (leftover check) plus the sandbox pid query.
        assert_eq!(count_arg(&lines, "state"), 4);
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

    #[test]
    fn setid_bits_follow_root_and_sudo() {
        assert_eq!(mask_mode(0o4755, false), 0o755);
        assert_eq!(mask_mode(0o2755, false), 0o755);
        assert_eq!(mask_mode(0o6755, false), 0o755);
        assert_eq!(mask_mode(0o1755, false), 0o1755);
        assert_eq!(mask_mode(0o4755, true), 0o4755);
        assert_eq!(mask_mode(0o2755, true), 0o2755);
        assert_eq!(mask_mode(0o1777, true), 0o1777);
    }

    #[tokio::test]
    async fn unpack_keeps_layer_file_modes() {
        let _guard = env_lock().lock().await;
        let _euid = EnvSlot::set("PISTACK_YOUKI_TEST_EUID", "1000");
        let _sudo = EnvSlot::clear(SUDO_ENV);
        assert!(!keep_setid());

        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-modes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = TreeGuard(tmp.clone());

        let layer = tar_nodes(&[
            ("rodir", b"", b'5', 0o555, ""),
            ("rodir/inside", b"in", b'0', 0o644, ""),
            ("bin/httpd", b"httpd", b'0', 0o755, ""),
            ("etc/config", b"cfg", b'0', 0o644, ""),
            ("bin/suid", b"s", b'0', 0o4755, ""),
            ("usr/bin/tool", b"tool", b'0', 0o755, ""),
            ("linkbin", b"", b'2', 0o777, "usr/bin"),
            ("linkbin/extra", b"ex", b'0', 0o644, ""),
            ("bin/httpd-hl", b"", b'1', 0o755, "bin/httpd"),
        ]);

        let tar_path = tmp.join("layer.tar");
        std::fs::write(&tar_path, &layer).unwrap();
        let root = tmp.join("root");
        std::fs::create_dir(&root).unwrap();
        unpack_image(&tar_path, &root).unwrap();
        assert_layer_modes(&root);

        let oci = tmp.join("oci");
        let blobs = oci.join("blobs").join("sha256");
        std::fs::create_dir_all(&blobs).unwrap();
        let manifest = "aa".repeat(32);
        let config = "bb".repeat(32);
        let layer_hex = "cc".repeat(32);
        std::fs::write(
            oci.join("oci-layout"),
            b"{\"imageLayoutVersion\":\"1.0.0\"}",
        )
        .unwrap();
        std::fs::write(
            oci.join("index.json"),
            format!(r#"{{"manifests":[{{"digest":"sha256:{manifest}"}}]}}"#),
        )
        .unwrap();
        std::fs::write(
            blobs.join(&manifest),
            format!(
                r#"{{"config":{{"digest":"sha256:{config}"}},"layers":[{{"mediaType":"application/vnd.oci.image.layer.v1.tar","digest":"sha256:{layer_hex}"}}]}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            blobs.join(&config),
            br#"{"config":{"Entrypoint":["/bin/httpd"]}}"#,
        )
        .unwrap();
        std::fs::write(blobs.join(&layer_hex), &layer).unwrap();
        let oci_root = tmp.join("oci-root");
        std::fs::create_dir(&oci_root).unwrap();
        let cfg = unpack_image(&oci, &oci_root).unwrap();
        assert_eq!(cfg.entrypoint, vec!["/bin/httpd".to_string()]);
        assert_layer_modes(&oci_root);

        // A rootless agent owns these directories, including mode 0555, and
        // must be able to remove them without the cleanup helper.
        let rootless = Privilege::Rootless {
            uid: 1000,
            gid: 1000,
        };
        delete_path(&root, &rootless).unwrap();
        assert!(!root.exists());
        delete_path(&oci_root, &rootless).unwrap();
        assert!(!oci_root.exists());

        let outside = PathBuf::from(format!(
            "/tmp/yk-out-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        let _outside = TreeGuard(outside.clone());
        let link = outside.to_str().unwrap();
        assert!(link.len() < 100, "{link}");
        assert!(link.starts_with('/'));
        assert!(!link.contains(".."));
        let bad = tar_nodes(&[
            ("jump", b"", b'2', 0o777, link),
            ("jump/pwned", b"pwned", b'0', 0o644, ""),
        ]);
        let bad_tar = tmp.join("escape.tar");
        std::fs::write(&bad_tar, bad).unwrap();
        let bad_root = tmp.join("escape-root");
        std::fs::create_dir(&bad_root).unwrap();
        let err = unpack_image(&bad_tar, &bad_root).unwrap_err();
        assert!(err.message.contains("image cannot be unpacked"), "{err}");
        assert!(!outside.join("pwned").exists());
        assert!(!bad_root.join("jump").join("pwned").exists());

        std::env::set_var(SUDO_ENV, "1");
        assert!(keep_setid());
        std::env::remove_var(SUDO_ENV);
        std::env::set_var("PISTACK_YOUKI_TEST_EUID", "0");
        assert!(keep_setid());
    }

    #[test]
    fn directory_image_keeps_file_and_directory_modes() {
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-dirmode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = TreeGuard(tmp.clone());

        let src = tmp.join("src");
        let bin = src.join("bin");
        let ro = src.join("rodir");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&ro).unwrap();
        let chmod = |path: &Path, mode: u32| {
            let mut perms = std::fs::symlink_metadata(path).unwrap().permissions();
            perms.set_mode(mode);
            std::fs::set_permissions(path, perms).unwrap();
        };

        let tool = bin.join("tool");
        std::fs::write(&tool, b"#!/bin/sh\n").unwrap();
        chmod(&tool, 0o755);
        let secret = src.join("secret");
        std::fs::write(&secret, b"hidden").unwrap();
        chmod(&secret, 0o600);
        let inside = ro.join("inside");
        std::fs::write(&inside, b"in").unwrap();
        chmod(&inside, 0o644);
        chmod(&bin, 0o750);
        chmod(&ro, 0o555);

        let dst = tmp.join("dst");
        std::fs::create_dir(&dst).unwrap();
        unpack_image(&src, &dst).unwrap();

        assert_eq!(mode_of(&dst.join("bin").join("tool")), 0o755);
        assert_eq!(
            std::fs::read(dst.join("bin").join("tool")).unwrap(),
            b"#!/bin/sh\n"
        );
        assert_eq!(mode_of(&dst.join("secret")), 0o600);
        assert_eq!(std::fs::read(dst.join("secret")).unwrap(), b"hidden");
        assert_eq!(mode_of(&dst.join("bin")), 0o750);
        assert_eq!(mode_of(&dst.join("rodir")), 0o555);
        assert_eq!(mode_of(&dst.join("rodir").join("inside")), 0o644);
        assert_eq!(
            std::fs::read(dst.join("rodir").join("inside")).unwrap(),
            b"in"
        );
    }

    fn assert_layer_modes(root: &Path) {
        assert_eq!(mode_of(&root.join("bin").join("httpd")), 0o755);
        assert_eq!(
            std::fs::read(root.join("bin").join("httpd")).unwrap(),
            b"httpd"
        );
        assert_eq!(mode_of(&root.join("etc").join("config")), 0o644);
        assert_eq!(
            std::fs::read(root.join("etc").join("config")).unwrap(),
            b"cfg"
        );
        assert_eq!(mode_of(&root.join("rodir")), 0o555);
        assert_eq!(mode_of(&root.join("rodir").join("inside")), 0o644);
        assert_eq!(
            std::fs::read(root.join("rodir").join("inside")).unwrap(),
            b"in"
        );
        assert_eq!(mode_bits(&root.join("bin").join("suid")), 0o755);
        assert!(std::fs::symlink_metadata(root.join("linkbin"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_link(root.join("linkbin")).unwrap(),
            Path::new("usr/bin")
        );
        assert_eq!(
            std::fs::read(root.join("usr").join("bin").join("extra")).unwrap(),
            b"ex"
        );
        assert_eq!(mode_of(&root.join("usr").join("bin").join("extra")), 0o644);
        assert_eq!(
            std::fs::read(root.join("bin").join("httpd-hl")).unwrap(),
            b"httpd"
        );
        assert_eq!(mode_of(&root.join("bin").join("httpd-hl")), 0o755);
    }

    struct TreeGuard(PathBuf);
    impl Drop for TreeGuard {
        fn drop(&mut self) {
            chmod_writable(&self.0);
            let _ = std::fs::remove_dir_all(&self.0);
        }
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
            None,
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
                rbd_mapped: false,
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
        let mounted = tmp.join("slab-mount");
        std::fs::create_dir_all(&mounted).unwrap();
        let _mounts = inject_mountinfo(&tmp, &[mounted.as_path()]);
        let raw = tmp.join("slab-file");
        std::fs::write(&raw, "block").unwrap();
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
            },
            "volume_paths": { vol_id: mounted.to_string_lossy() }
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
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], mounted.to_string_lossy().as_ref());
        assert_eq!(
            data["options"],
            serde_json::json!(["rbind", "rprivate", "rw"])
        );
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
                path: raw.to_string_lossy().into_owned(),
                device: "vdb".into(),
                read_only: true,
                rbd_mapped: true,
            }],
        )
        .unwrap();
        refresh_disk_mounts(&bundle).unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], mounted.to_string_lossy().as_ref());
        assert_eq!(
            data["options"],
            serde_json::json!(["rbind", "rprivate", "ro"])
        );
        privileged
            .stage_pod_bundles(&bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], mounted.to_string_lossy().as_ref());
        assert_eq!(
            data["options"],
            serde_json::json!(["rbind", "rprivate", "ro"])
        );
        assert_ne!(data["source"], raw.to_string_lossy().as_ref());
        assert_eq!(
            web["process"]["args"],
            serde_json::json!(["/usr/bin/web", "--serve"])
        );
        assert_eq!(
            mode_of(&bundle.join("containers").join("0").join("config.json")),
            0o600
        );
    }

    #[tokio::test]
    async fn slab_bind_source_must_exist_before_create() {
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-slab-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let missing = tmp.join("missing-slab");
        let vol_id = "vol-22222222-2222-2222-2222-222222222222";
        let user_data = serde_json::json!({
            "kind": "PodSandbox",
            "containers": [{
                "name": "web",
                "volume_mounts": [{"name": "disk", "mount_path": "/data"}]
            }],
            "volumes": [{"name": "disk", "source": {"volume": vol_id}}],
            "volume_paths": { vol_id: missing.to_string_lossy() }
        })
        .to_string();
        let mut request = req("web", "/nope", "10.9.0.1");
        request.user_data = Some(user_data);
        let plan = sandbox_plan(&request).unwrap().unwrap();
        let bundle = tmp.join("pod");
        let err = RunMode::privileged()
            .stage_pod_bundles(
                &bundle,
                "web",
                &Limits {
                    vcpus: 1,
                    memory_mib: 64,
                },
                &plan,
                "/nope",
                1,
            )
            .unwrap_err();
        assert!(err.message.contains("does not exist"), "{err}");
        assert!(err.message.contains(missing.to_str().unwrap()), "{err}");
        assert!(!bundle.join("config.json").exists());
        assert!(!bundle.join("containers").exists());

        let bare = serde_json::json!({
            "kind": "PodSandbox",
            "containers": [{
                "name": "web",
                "volume_mounts": [{"name": "disk", "mount_path": "/data"}]
            }],
            "volumes": [{"name": "disk", "source": {"volume": vol_id}}]
        })
        .to_string();
        request.user_data = Some(bare);
        let plan = sandbox_plan(&request).unwrap().unwrap();
        let err = ensure_slab_sources(&plan, &[]).unwrap_err();
        assert!(err.message.contains("has no host path"), "{err}");
        assert!(err.message.contains("disk"), "{err}");
    }

    #[test]
    fn slab_source_errors_name_the_volume() {
        let missing = slab_source_error("disk", "/var/lib/pistack/vol", SlabSource::Missing);
        assert!(missing.message.contains("disk"), "{missing}");
        assert!(
            missing.message.contains("/var/lib/pistack/vol"),
            "{missing}"
        );
        assert!(missing.message.contains("does not exist"), "{missing}");

        let file = slab_source_error("disk", "/var/lib/pistack/disk", SlabSource::File);
        assert!(file.message.contains("disk"), "{file}");
        assert!(file.message.contains("/var/lib/pistack/disk"), "{file}");
        assert!(file.message.contains("is a file"), "{file}");
        assert!(file.message.contains("not a mounted directory"), "{file}");

        let block = slab_source_error("data", "/dev/rbd0", SlabSource::Block);
        assert!(block.message.contains("data"), "{block}");
        assert!(block.message.contains("/dev/rbd0"), "{block}");
        assert!(block.message.contains("block device"), "{block}");
        assert!(block.message.contains("not a mounted directory"), "{block}");
    }

    fn slab_plan(image: &str, vol_id: &str, path: Option<&str>, device: Option<&str>) -> PodPlan {
        let mut volume_paths = BTreeMap::new();
        if let Some(path) = path {
            volume_paths.insert(vol_id.to_string(), path.to_string());
        }
        let mut volume_devices = BTreeMap::new();
        if let Some(device) = device {
            volume_devices.insert(vol_id.to_string(), device.to_string());
        }
        PodPlan {
            containers: vec![SandboxContainer {
                name: "web".into(),
                image: image.into(),
                volume_mounts: vec![SandboxMount {
                    name: "disk".into(),
                    mount_path: "/data".into(),
                    read_only: false,
                    sub_path: String::new(),
                }],
                ..SandboxContainer::default()
            }],
            volumes: vec![SandboxVolume {
                name: "disk".into(),
                source: VolumeSrc::Volume(vol_id.into()),
            }],
            config_maps: BTreeMap::new(),
            secrets: BTreeMap::new(),
            volume_paths,
            volume_devices,
        }
    }

    /// Mount table the slab checks read instead of `/proc/self/mountinfo`.
    fn inject_mountinfo(dir: &Path, mounts: &[&Path]) -> EnvSlot {
        let file = dir.join("mountinfo");
        let mut text = String::new();
        for (index, mount) in mounts.iter().enumerate() {
            let id = index + 11;
            let point = mount.to_string_lossy().replace(' ', "\\040");
            text.push_str(&format!(
                "{id} 1 8:{id} / {point} rw - ext4 /dev/slab{id} rw\n"
            ));
        }
        std::fs::write(&file, text).unwrap();
        EnvSlot::set(MOUNTINFO_ENV, file.to_str().expect("utf8"))
    }

    fn attached_raw(vol_id: &str, path: &str, read_only: bool) -> AttachedDisk {
        AttachedDisk {
            volume: vol_id.into(),
            path: path.into(),
            device: "vdb".into(),
            read_only,
            rbd_mapped: true,
        }
    }

    #[tokio::test]
    async fn slab_mount_binds_the_recorded_folder() {
        let _guard = env_lock().lock().await;
        let _netns = EnvSlot::clear(NETNS_ENV);
        let tmp = temp_dir("slab-folder");
        let _dir = scopeguard_dir(&tmp);
        let image = tmp.join("image");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("marker"), "a").unwrap();
        let folder = tmp.join("mounted");
        std::fs::create_dir_all(&folder).unwrap();
        let _mounts = inject_mountinfo(&tmp, &[folder.as_path()]);
        let raw = tmp.join("disk.raw");
        std::fs::write(&raw, b"block").unwrap();
        let vol_id = "vol-33333333-3333-3333-3333-333333333333";
        let folder_s = folder.to_string_lossy().into_owned();
        let raw_s = raw.to_string_lossy().into_owned();
        let image_s = image.to_string_lossy().into_owned();
        let plan = slab_plan(&image_s, vol_id, Some(&folder_s), None);
        let disks = vec![attached_raw(vol_id, &raw_s, false)];
        let bundle = tmp.join("pod");
        let binds = pod_binds(&bundle, &plan, &plan.containers[0].volume_mounts, &disks).unwrap();
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].source, folder_s);
        assert!(binds[0].directory);
        assert!(!binds[0].read_only);
        assert_ne!(binds[0].source, raw_s);

        let limits = Limits {
            vcpus: 1,
            memory_mib: 64,
        };
        RunMode::privileged()
            .stage_pod_bundles(&bundle, "web", &limits, &plan, &image_s, 7)
            .unwrap();
        write_disks(&bundle, &disks).unwrap();
        let late = vec![attached_raw(vol_id, &raw_s, true)];
        write_disks(&bundle, &late).unwrap();
        refresh_disk_mounts(&bundle).unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], folder_s);
        assert_eq!(
            data["options"],
            serde_json::json!(["rbind", "rprivate", "ro"])
        );

        RunMode::privileged()
            .stage_pod_bundles(&bundle, "web", &limits, &plan, &image_s, 7)
            .unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], folder_s);
        assert_eq!(
            data["options"],
            serde_json::json!(["rbind", "rprivate", "ro"])
        );

        let file_plan = slab_plan(&image_s, vol_id, Some(&raw_s), None);
        let err = match pod_binds(
            &bundle,
            &file_plan,
            &file_plan.containers[0].volume_mounts,
            &[],
        ) {
            Err(err) => err,
            Ok(_) => panic!("a raw file was bound"),
        };
        assert!(err.message.contains("disk"), "{err}");
        assert!(err.message.contains(&raw_s), "{err}");
        assert!(err.message.contains("is a file"), "{err}");
        assert!(err.message.contains("not a mounted directory"), "{err}");

        let backing = match pod_binds(
            &bundle,
            &slab_plan(&image_s, vol_id, None, None),
            &plan.containers[0].volume_mounts,
            &disks,
        ) {
            Err(err) => err,
            Ok(_) => panic!("the backing file was bound"),
        };
        assert!(backing.message.contains("disk"), "{backing}");
        assert!(backing.message.contains(&raw_s), "{backing}");
        assert!(backing.message.contains("is a file"), "{backing}");

        let block = slab_source_error("disk", "/dev/vg/lv", SlabSource::Block);
        assert!(block.message.contains("disk"), "{block}");
        assert!(block.message.contains("block device"), "{block}");

        for path in ["/dev/disk0", "/dev/sda", "/dev/vda", "/dev/nvme0n1"] {
            if classify_slab_path(path).ok() != Some(SlabSource::Block) {
                continue;
            }
            let plan = slab_plan(&image_s, vol_id, Some(path), None);
            let err = match pod_binds(&bundle, &plan, &plan.containers[0].volume_mounts, &[]) {
                Err(err) => err,
                Ok(_) => panic!("block device {path} was bound"),
            };
            assert!(err.message.contains("disk"), "{err}");
            assert!(err.message.contains(path), "{err}");
            assert!(err.message.contains("block device"), "{err}");
            break;
        }
    }

    #[tokio::test]
    async fn late_attach_and_recreate_bind_the_mounted_folder() {
        let _guard = env_lock().lock().await;
        let _netns = EnvSlot::clear(NETNS_ENV);
        let tmp = temp_dir("slab-late");
        let _dir = scopeguard_dir(&tmp);
        let image = tmp.join("image");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("marker"), "a").unwrap();
        let folder = tmp.join("mounted");
        std::fs::create_dir_all(&folder).unwrap();
        let _mounts = inject_mountinfo(&tmp, &[folder.as_path()]);
        let raw = tmp.join("disk.raw");
        std::fs::write(&raw, b"block").unwrap();
        let vol_id = "vol-44444444-4444-4444-4444-444444444444";
        let folder_s = folder.to_string_lossy().into_owned();
        let raw_s = raw.to_string_lossy().into_owned();
        let image_s = image.to_string_lossy().into_owned();
        let waiting = slab_plan(&image_s, vol_id, None, Some("vdb"));
        let bundle = tmp.join("pod");
        let limits = Limits {
            vcpus: 1,
            memory_mib: 64,
        };
        RunMode::privileged()
            .stage_pod_bundles(&bundle, "web", &limits, &waiting, &image_s, 7)
            .unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        assert!(web["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|mount| mount["destination"] != "/data"));

        write_disks(&bundle, &[attached_raw(vol_id, &raw_s, false)]).unwrap();
        refresh_disk_mounts(&bundle).unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        assert!(web["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|mount| mount["destination"] != "/data"));

        let ready = slab_plan(&image_s, vol_id, Some(&folder_s), None);
        let disks = read_disks(&bundle).unwrap();
        let binds = pod_binds(&bundle, &ready, &ready.containers[0].volume_mounts, &disks).unwrap();
        assert_eq!(binds[0].source, folder_s);
        assert!(binds[0].directory);
        RunMode::privileged()
            .stage_pod_bundles(&bundle, "web", &limits, &ready, &image_s, 7)
            .unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], folder_s);
        assert_eq!(
            data["options"],
            serde_json::json!(["rbind", "rprivate", "rw"])
        );

        RunMode::privileged()
            .stage_pod_bundles(&bundle, "web", &limits, &ready, &image_s, 7)
            .unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], folder_s);
        assert_eq!(
            data["options"],
            serde_json::json!(["rbind", "rprivate", "rw"])
        );
    }

    #[test]
    fn mountinfo_matches_the_exact_path_and_unescapes_spaces() {
        let text = "\
36 35 98:0 / /mnt/data rw,noatime master:1 - ext4 /dev/root rw\n\
37 35 98:0 / /mnt/my\\040vol rw - ext4 /dev/sda1 rw\n\
38 35 98:0 / /mnt/data-extra rw - ext4 /dev/sdb1 rw\n\
50 1 8:3 / /mnt/elsewhere rw - ext4 /var/lib/pistack/vol rw\n";
        assert!(mountinfo_lists(text, "/mnt/data"));
        assert!(mountinfo_lists(text, "/mnt/data/"));
        assert!(mountinfo_lists(text, "/mnt/my vol"));
        assert!(!mountinfo_lists(text, "/mnt"));
        assert!(!mountinfo_lists(text, "/mnt/data-extra/child"));
        assert!(!mountinfo_lists(text, "/mnt/missing"));
        // The device column is not a mount point.
        assert!(!mountinfo_lists(text, "/var/lib/pistack/vol"));
        assert_eq!(unescape_mount("my\\040vol"), "my vol");
        assert_eq!(unescape_mount("line\\012break"), "line\nbreak");
    }

    #[tokio::test]
    async fn mounted_slab_folder_is_accepted() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("slab-mounted");
        let _dir = scopeguard_dir(&tmp);
        let image = tmp.join("image");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("marker"), "a").unwrap();
        let folder = tmp.join("my vol");
        std::fs::create_dir_all(&folder).unwrap();
        let _mounts = inject_mountinfo(&tmp, &[folder.as_path()]);
        let vol_id = "vol-55555555-5555-5555-5555-555555555555";
        let folder_s = folder.to_string_lossy().into_owned();
        let image_s = image.to_string_lossy().into_owned();
        let plan = slab_plan(&image_s, vol_id, Some(&folder_s), None);
        ensure_slab_sources(&plan, &[]).unwrap();
        let bundle = tmp.join("pod");
        let binds = pod_binds(&bundle, &plan, &plan.containers[0].volume_mounts, &[]).unwrap();
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].source, folder_s);
        assert!(binds[0].directory);
        let limits = Limits {
            vcpus: 1,
            memory_mib: 64,
        };
        RunMode::privileged()
            .stage_pod_bundles(&bundle, "web", &limits, &plan, &image_s, 7)
            .unwrap();
        let web: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let data = mount_at(&web, "/data");
        assert_eq!(data["source"], folder_s);
        assert_eq!(
            data["options"],
            serde_json::json!(["rbind", "rprivate", "rw"])
        );
    }

    #[tokio::test]
    async fn unmounted_slab_folder_is_refused() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("slab-unmounted");
        let _dir = scopeguard_dir(&tmp);
        let image = tmp.join("image");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("marker"), "a").unwrap();
        let empty = tmp.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let table = tmp.join("mountinfo");
        let parent = empty
            .parent()
            .unwrap()
            .to_string_lossy()
            .replace(' ', "\\040");
        std::fs::write(
            &table,
            format!("41 1 8:1 / {parent} rw - ext4 /dev/disk rw\n"),
        )
        .unwrap();
        let _mounts = EnvSlot::set(MOUNTINFO_ENV, table.to_str().expect("utf8"));
        let vol_id = "vol-66666666-6666-6666-6666-666666666666";
        let empty_s = empty.to_string_lossy().into_owned();
        let image_s = image.to_string_lossy().into_owned();
        let plan = slab_plan(&image_s, vol_id, Some(&empty_s), None);
        let expected = format!("slab volume disk is not mounted at {empty_s}");
        let err = ensure_slab_sources(&plan, &[]).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains(&expected), "{err}");
        let bundle = tmp.join("pod");
        let limits = Limits {
            vcpus: 1,
            memory_mib: 64,
        };
        let err = RunMode::privileged()
            .stage_pod_bundles(&bundle, "web", &limits, &plan, &image_s, 7)
            .unwrap_err();
        assert!(err.message.contains(&expected), "{err}");
        assert!(!bundle.join("config.json").exists());
        assert!(!bundle.join("containers").exists());

        let disks = vec![attached_raw(vol_id, &empty_s, false)];
        assert!(oci_binds(
            &bundle,
            &plan.volumes,
            &plan.containers[0].volume_mounts,
            &disks,
            &plan.volume_paths,
        )
        .is_empty());

        // The mount table cannot be read, so the device-number fallback
        // still refuses a folder that shares its parent's device.
        let missing = tmp.join("no-such-mountinfo");
        let _fallback = EnvSlot::set(MOUNTINFO_ENV, missing.to_str().expect("utf8"));
        let err = match pod_binds(&bundle, &plan, &plan.containers[0].volume_mounts, &[]) {
            Err(err) => err,
            Ok(_) => panic!("an unmounted folder was bound"),
        };
        assert!(err.message.contains(&expected), "{err}");
        let late = slab_plan(&image_s, vol_id, Some(&empty_s), None);
        let err = RunMode::privileged()
            .stage_pod_bundles(&bundle, "web", &limits, &late, &image_s, 7)
            .unwrap_err();
        assert!(err.message.contains(&expected), "{err}");
    }

    #[tokio::test]
    async fn sudo_mode_opens_secret_volumes_and_keeps_the_bundle_private() {
        let _guard = env_lock().lock().await;
        let _netns = EnvSlot::clear(NETNS_ENV);
        let tmp = temp_dir("sudo-vol");
        let _dir = scopeguard_dir(&tmp);
        let image = tmp.join("image");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("marker"), "a").unwrap();
        let user_data = serde_json::json!({
            "kind": "PodSandbox",
            "containers": [{
                "name": "web",
                "image": image,
                "command": ["/bin/web"]
            }],
            "volumes": [
                {"name": "cfg", "source": {"config_map": "app"}},
                {"name": "sec", "source": {"secret": "db"}},
                {"name": "empty-sec", "source": {"secret": "missing"}}
            ],
            "config_maps": {"app": {"data": {"MODE": "prod"}}},
            "secrets": {"db": {"data": {"password": "s3cret-value"}}}
        })
        .to_string();
        let mut request = req("web", "not-a-path", "10.9.1.1");
        request.user_data = Some(user_data);
        let plan = sandbox_plan(&request).unwrap().unwrap();
        let limits = Limits {
            vcpus: 1,
            memory_mib: 64,
        };
        let sudo_mode = RunMode {
            privilege: Privilege::Sudo {
                sudo: PathBuf::from("/bin/sudo"),
            },
            cgroup: CgroupCaps {
                memory: true,
                cpu: true,
            },
        };
        let sudo_bundle = tmp.join("sudo");
        sudo_mode
            .stage_pod_bundles(&sudo_bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();
        assert_eq!(mode_of(&sudo_bundle), 0o700);
        let cfg = sudo_bundle.join("volumes").join("0-cfg");
        let sec = sudo_bundle.join("volumes").join("1-sec");
        let empty = sudo_bundle.join("volumes").join("2-empty-sec");
        assert_eq!(mode_of(&cfg), 0o755);
        assert_eq!(mode_of(&cfg.join("MODE")), 0o644);
        assert_eq!(std::fs::read_to_string(cfg.join("MODE")).unwrap(), "prod");
        assert_eq!(mode_of(&sec), 0o755);
        assert_eq!(mode_of(&sec.join("password")), 0o644);
        assert_eq!(
            std::fs::read_to_string(sec.join("password")).unwrap(),
            "s3cret-value"
        );
        assert_eq!(mode_of(&empty), 0o755);
        assert_eq!(
            file_uid(&sec.join("password")).unwrap(),
            file_uid(&image.join("marker")).unwrap()
        );

        let root_bundle = tmp.join("root");
        RunMode::privileged()
            .stage_pod_bundles(&root_bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();
        assert_eq!(mode_of(&root_bundle.join("volumes").join("0-cfg")), 0o755);
        assert_eq!(
            mode_of(&root_bundle.join("volumes").join("0-cfg").join("MODE")),
            0o644
        );
        assert_eq!(mode_of(&root_bundle.join("volumes").join("1-sec")), 0o700);
        assert_eq!(
            mode_of(&root_bundle.join("volumes").join("1-sec").join("password")),
            0o400
        );
        assert_eq!(
            mode_of(&root_bundle.join("volumes").join("2-empty-sec")),
            0o700
        );

        let less_bundle = tmp.join("rootless");
        RunMode::rootless(1000, 1000)
            .stage_pod_bundles(&less_bundle, "web", &limits, &plan, "not-a-path", 4242)
            .unwrap();
        assert_eq!(mode_of(&less_bundle.join("volumes").join("1-sec")), 0o700);
        assert_eq!(
            mode_of(&less_bundle.join("volumes").join("1-sec").join("password")),
            0o400
        );
        assert_eq!(
            mode_of(&less_bundle.join("volumes").join("0-cfg").join("MODE")),
            0o644
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
            fs_type: String::new(),
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

    fn write_executable(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    /// Stub `rbd`. `device list` and `showmapped` print the mapped image.
    /// `device map` records it and prints `/dev/rbd0`. `device unmap` clears it.
    fn write_rbd(path: &Path, log: &Path, state: &Path) {
        let log_q = shell_quote(log);
        let state_q = shell_quote(state);
        let body = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" >> {log_q}
printf '\n' >> {log_q}
if [ "$1" = device ] && [ "$2" = list ]; then
  if [ -f {state_q} ]; then cat {state_q}; else printf '%s\n' '[]'; fi
  exit 0
fi
if [ "$1" = showmapped ]; then
  if [ -f {state_q} ]; then cat {state_q}; else printf '%s\n' '[]'; fi
  exit 0
fi
if [ "$1" = device ] && [ "$2" = map ]; then
  spec=$3
  pool=${{spec%%/*}}
  image=${{spec#*/}}
  printf '[{{"id":"0","pool":"%s","namespace":"","name":"%s","snap":"-","device":"/dev/rbd0"}}]\n' "$pool" "$image" > {state_q}
  printf '%s\n' /dev/rbd0
  exit 0
fi
if [ "$1" = device ] && [ "$2" = unmap ]; then
  printf '%s\n' '[]' > {state_q}
  exit 0
fi
echo unexpected >&2
exit 1
"#,
            log_q = log_q,
            state_q = state_q,
        );
        write_executable(path, &body);
    }

    fn rbd_invocations(log: &str) -> Vec<Vec<String>> {
        log.split("\n\n")
            .map(|block| {
                block
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter(|block| !block.is_empty())
            .collect()
    }

    fn count_rbd(inv: &[Vec<String>], first: &str, second: &str) -> usize {
        inv.iter()
            .filter(|args| {
                args.first().map(String::as_str) == Some(first)
                    && args.get(1).map(String::as_str) == Some(second)
            })
            .count()
    }

    fn rbd_target(keyring: Option<String>) -> pistack_rpc::proto::DiskTarget {
        pistack_rpc::proto::DiskTarget::Rbd {
            pool: "volumes".into(),
            image: "img-1".into(),
            monitors: vec!["10.1.0.5:6789".into(), "mon.example".into()],
            user: "pistack".into(),
            key_secret: keyring,
        }
    }

    fn rbd_attach(
        machine: Id,
        volume: Id,
        target: pistack_rpc::proto::DiskTarget,
    ) -> crate::runtime::VolumeAttach {
        crate::runtime::VolumeAttach {
            machine,
            volume,
            path: String::new(),
            device: None,
            read_only: true,
            fs_type: String::new(),
            target: Some(target),
        }
    }

    fn write_pod_mount(bundle: &Path, volume: &str) {
        let plan = serde_json::json!({
            "volumes": [{"name": "data", "source": {"volume": volume}}],
            "containers": [{
                "index": 0,
                "mounts": [{
                    "name": "data",
                    "mount_path": "/data",
                    "read_only": false,
                    "sub_path": ""
                }]
            }]
        });
        std::fs::write(
            bundle.join("mount-plan.json"),
            serde_json::to_vec_pretty(&plan).unwrap(),
        )
        .unwrap();
        let dir = bundle.join("containers").join("0");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            br#"{"mounts":[{"destination":"/proc","type":"proc","source":"proc"}]}"#,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn rbd_pod_maps_a_host_device_and_unmaps_on_detach() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-rbd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let rbd_log = tmp.join("rbd.log");
        let rbd_state = tmp.join("mapped.json");
        let rbd = tmp.join("rbd");
        write_rbd(&rbd, &rbd_log, &rbd_state);
        let _env = EnvGuard::set(&tmp.join("youki"), &tmp.join("state"));
        std::env::set_var("PISTACK_RBD", &rbd);
        std::env::set_var("PISTACK_RBD_TEST_EUID", "0");

        let rt = YoukiRuntime;
        let machine = Id::new(kind::MACHINE);
        let volume = Id::new(kind::VOLUME);
        let secret = "super-secret-value";
        let missing = rbd_attach(
            machine.clone(),
            volume.clone(),
            rbd_target(Some(secret.into())),
        );
        assert_eq!(
            rt.attach_volume(&missing).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert!(!rbd_log.exists());

        let bundle = tmp.join("state").join(machine.to_string());
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("config.json"), b"{}").unwrap();
        write_pod_mount(&bundle, &volume.to_string());
        let refused = rt.attach_volume(&missing).await.unwrap_err();
        assert_eq!(refused.code, ErrorCode::Unavailable);
        assert!(!refused.message.contains(secret));
        assert!(!rbd_log.exists());

        let keyring = tmp.join("keyring");
        std::fs::write(&keyring, secret).unwrap();
        let attach = rbd_attach(
            machine.clone(),
            volume.clone(),
            rbd_target(Some(keyring.to_string_lossy().into_owned())),
        );
        assert_eq!(rt.attach_volume(&attach).await.unwrap(), "vdb");
        let disks = read_disks(&bundle).unwrap();
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].path, "/dev/rbd0");
        assert!(disks[0].rbd_mapped);
        assert_eq!(disks[0].device, "vdb");
        let mounted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        assert!(mounted["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|mount| mount["destination"] != "/data"));

        assert_eq!(rt.attach_volume(&attach).await.unwrap(), "vdb");
        assert_eq!(read_disks(&bundle).unwrap()[0].path, "/dev/rbd0");
        let inv = rbd_invocations(&std::fs::read_to_string(&rbd_log).unwrap());
        assert_eq!(count_rbd(&inv, "device", "map"), 1);
        assert!(count_rbd(&inv, "device", "list") >= 2);
        let map = inv
            .iter()
            .find(|args| {
                args.first().map(String::as_str) == Some("device")
                    && args.get(1).map(String::as_str) == Some("map")
            })
            .unwrap();
        assert!(map.iter().any(|arg| arg == "volumes/img-1"));
        assert!(map.iter().any(|arg| arg == "--id"));
        assert!(map.iter().any(|arg| arg == "pistack"));
        assert!(map.iter().any(|arg| arg == "-m"));
        assert!(map
            .iter()
            .any(|arg| arg == "10.1.0.5:6789,mon.example:6789"));
        assert!(map.iter().any(|arg| arg == "--keyring"));
        assert!(map.iter().any(|arg| arg == keyring.to_str().unwrap()));
        assert!(map.iter().any(|arg| arg == "--read-only"));
        let logged = std::fs::read_to_string(&rbd_log).unwrap();
        assert!(!logged.contains(secret));

        rt.detach_volume(&crate::runtime::VolumeDetach {
            machine: machine.clone(),
            volume: volume.clone(),
            path: String::new(),
            device: None,
            target: Some(rbd_target(Some(keyring.to_string_lossy().into_owned()))),
        })
        .await
        .unwrap();
        assert!(read_disks(&bundle).unwrap().is_empty());
        let inv = rbd_invocations(&std::fs::read_to_string(&rbd_log).unwrap());
        assert_eq!(count_rbd(&inv, "device", "unmap"), 1);
        assert_eq!(std::fs::read_to_string(&rbd_state).unwrap().trim(), "[]");

        let dev_root = tmp.join("dev");
        let stable = dev_root.join("rbd").join("volumes").join("img-1");
        std::fs::create_dir_all(stable.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/dev/rbd0", &stable).unwrap();
        std::env::set_var("PISTACK_RBD_DEV_ROOT", &dev_root);
        assert_eq!(rt.attach_volume(&attach).await.unwrap(), "vdb");
        assert_eq!(
            read_disks(&bundle).unwrap()[0].path,
            stable.to_string_lossy().as_ref()
        );
        let mounted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(bundle.join("containers").join("0").join("config.json")).unwrap(),
        )
        .unwrap();
        let stable_s = stable.to_string_lossy();
        assert!(mounted["mounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|mount| mount["source"] != stable_s.as_ref()));
    }

    #[tokio::test]
    async fn rbd_sudo_maps_for_a_non_root_agent_and_delete_unmaps() {
        let _guard = env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-rbd-sudo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let _dir = scopeguard_dir(&tmp);
        let youki_log = tmp.join("youki.log");
        let youki = tmp.join("youki");
        write_script(&youki, &youki_log, 0);
        let rbd_log = tmp.join("rbd.log");
        let rbd_state = tmp.join("mapped.json");
        let rbd = tmp.join("rbd");
        write_rbd(&rbd, &rbd_log, &rbd_state);
        let sudo_log = tmp.join("sudo.log");
        let sudo = tmp.join("sudo");
        write_sudo(&sudo, &sudo_log);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        std::env::set_var("PISTACK_RBD", &rbd);
        std::env::set_var("PISTACK_RBD_SUDO", "1");
        std::env::set_var("PISTACK_RBD_SUDO_BIN", &sudo);
        std::env::set_var("PISTACK_RBD_TEST_EUID", "1000");

        let keyring = tmp.join("keyring");
        std::fs::write(&keyring, b"key-bytes").unwrap();
        let rt = YoukiRuntime;
        let machine = Id::new(kind::MACHINE);
        let volume = Id::new(kind::VOLUME);
        let bundle = state.join(machine.to_string());
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("config.json"), b"{}").unwrap();
        let attach = rbd_attach(
            machine.clone(),
            volume.clone(),
            rbd_target(Some(keyring.to_string_lossy().into_owned())),
        );
        assert_eq!(rt.attach_volume(&attach).await.unwrap(), "vdb");
        assert_eq!(read_disks(&bundle).unwrap()[0].path, "/dev/rbd0");

        let rbd_arg = rbd.to_string_lossy().into_owned();
        let sudo_lines = arg_lines(&std::fs::read_to_string(&sudo_log).unwrap());
        assert!(
            sudo_lines
                .windows(4)
                .any(|w| { w[0] == "-n" && w[1] == rbd_arg && w[2] == "device" && w[3] == "map" }),
            "map was not run through sudo: {sudo_lines:?}"
        );
        assert!(sudo_lines
            .iter()
            .all(|line| line != "list" && line != "showmapped"));
        let direct = rbd_invocations(&std::fs::read_to_string(&rbd_log).unwrap());
        assert!(count_rbd(&direct, "device", "list") >= 1);

        rt.delete(&machine).await.unwrap();
        assert!(!bundle.exists());
        let sudo_lines = arg_lines(&std::fs::read_to_string(&sudo_log).unwrap());
        assert!(
            sudo_lines.windows(4).any(|w| {
                w[0] == "-n" && w[1] == rbd_arg && w[2] == "device" && w[3] == "unmap"
            }),
            "unmap was not run through sudo: {sudo_lines:?}"
        );

        std::env::set_var("PISTACK_RBD_TEST_EUID", "0");
        let _ = std::fs::write(&sudo_log, b"");
        let _ = std::fs::write(&rbd_log, b"");
        let root_machine = Id::new(kind::MACHINE);
        let root_bundle = state.join(root_machine.to_string());
        std::fs::create_dir_all(&root_bundle).unwrap();
        std::fs::write(root_bundle.join("config.json"), b"{}").unwrap();
        let root_attach = rbd_attach(
            root_machine,
            volume,
            rbd_target(Some(keyring.to_string_lossy().into_owned())),
        );
        assert_eq!(rt.attach_volume(&root_attach).await.unwrap(), "vdb");
        assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), "");
        let direct = rbd_invocations(&std::fs::read_to_string(&rbd_log).unwrap());
        assert_eq!(count_rbd(&direct, "device", "map"), 1);
        assert!(direct.iter().all(|args| args.iter().all(|arg| arg != "-n")));
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
        let found = lines.windows(2).enumerate().any(|(i, w)| {
            if w[0] != "-n" || w[1] != youki {
                return false;
            }
            lines[i + 2..]
                .iter()
                .take_while(|line| line.as_str() != "-n")
                .any(|line| line == op)
        });
        assert!(found, "missing sudo {op}: {lines:?}");
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

    #[test]
    fn youki_cleanup_helper_self_test() {
        // SAFETY: geteuid does not fail and does not touch Rust memory.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let script =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/pistack-youki-cleanup");
        let output = std::process::Command::new("python3")
            .arg("-c")
            .arg("import runpy, sys; runpy.run_path(sys.argv[1])['self_test']()")
            .arg(&script)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "helper self-test failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn sudo_mode_removes_a_bundle_the_agent_cannot_unlink() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("sudo-rm");
        let _dir = scopeguard_dir(&tmp);
        let youki_log = tmp.join("youki.log");
        let sudo_log = tmp.join("sudo.log");
        let youki = tmp.join("youki");
        let sudo_bin = tmp.join("sudo");
        write_script(&youki, &youki_log, 0);
        write_sudo(&sudo_bin, &sudo_log);
        let helper = install_cleanup_helper(&tmp);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        std::env::set_var("PISTACK_YOUKI_TEST_EUID", "1000");
        std::env::set_var("PISTACK_YOUKI_TEST_EGID", "1000");
        std::env::set_var(SUDO_ENV, "1");
        std::env::set_var(SUDO_BIN_ENV, &sudo_bin);
        std::env::set_var(CLEANUP_ENV, &helper);
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");
        let privilege = Privilege::Sudo {
            sudo: sudo_bin.clone(),
        };

        let outside = tmp.join("outside");
        std::fs::write(&outside, b"keep").unwrap();
        let sibling = state.join("sibling-keep");
        std::fs::create_dir_all(&sibling).unwrap();
        std::fs::write(sibling.join("file"), b"keep").unwrap();

        let rootfs = state.join("mach-1").join("rootfs");
        let locked_rootfs = rootfs.join("locked");
        lock_dir(&locked_rootfs);
        {
            let _unlock = Unlock(locked_rootfs.clone());
            assert!(std::fs::remove_file(locked_rootfs.join("file")).is_err());
            reset_dir(&rootfs, &privilege).unwrap();
            assert!(rootfs.is_dir());
            assert!(!locked_rootfs.exists());
            assert!(std::fs::read_dir(&rootfs).unwrap().next().is_none());
        }
        assert_eq!(std::fs::read(sibling.join("file")).unwrap(), b"keep");
        let helper_arg = helper.to_string_lossy().into_owned();
        let sudo_text = std::fs::read_to_string(&sudo_log).unwrap();
        assert!(
            sudo_text.contains(&helper_arg),
            "rootfs removal did not use the helper: {sudo_text}"
        );
        assert!(
            !sudo_text.contains(".."),
            "privileged removal saw a dot-dot path: {sudo_text}"
        );

        let image = tmp.join("image");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("marker"), b"keep").unwrap();
        let rt = YoukiRuntime;
        let request = req("web", image.to_str().unwrap(), "10.8.1.1");
        rt.start(&request).await.unwrap();
        let bundle = state.join(request.id.to_string());
        assert_eq!(mode_of(&bundle), 0o700);
        let locked = bundle.join("rootfs").join("locked");
        lock_dir(&locked);
        let _unlock = Unlock(locked.clone());
        assert!(std::fs::remove_file(locked.join("file")).is_err());
        rt.stop(&request.id).await.unwrap();
        rt.delete(&request.id).await.unwrap();
        assert!(!bundle.exists());
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
        assert_eq!(std::fs::read(sibling.join("file")).unwrap(), b"keep");
        let sudo_text = std::fs::read_to_string(&sudo_log).unwrap();
        let bundle_arg = bundle.to_string_lossy().into_owned();
        assert!(
            sudo_text.contains(&bundle_arg),
            "delete did not remove the bundle through sudo: {sudo_text}"
        );
        let lines = arg_lines(&sudo_text);
        assert!(
            lines.windows(4).any(|w| {
                w[0] == "-n" && w[1] == helper_arg && w[2] == "delete" && w[3] == "--path"
            }),
            "{lines:?}"
        );
        let youki_lines = arg_lines(&std::fs::read_to_string(&youki_log).unwrap());
        assert!(
            youki_lines
                .windows(2)
                .any(|w| w[0] == "delete" && w[1] == "--force"),
            "{youki_lines:?}"
        );

        let _ = std::fs::write(&sudo_log, b"");
        let plain = req("plain", image.to_str().unwrap(), "10.8.1.2");
        rt.start(&plain).await.unwrap();
        rt.delete(&plain.id).await.unwrap();
        let sudo_text = std::fs::read_to_string(&sudo_log).unwrap();
        assert!(
            !sudo_text.contains(&helper_arg),
            "agent-owned bundle used the helper: {sudo_text}"
        );
        assert!(sudo_text.contains("delete"), "{sudo_text}");

        let cause = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let _ = std::fs::write(&sudo_log, b"");
        let tricky = state
            .join("..")
            .join(tmp.file_name().unwrap())
            .join("outside");
        let err = privileged_remove(&tricky, &privilege, &cause).unwrap_err();
        assert!(err.message.contains("Refusing privileged removal"), "{err}");
        assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), "");
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
        assert!(checked_removal_path(&state.join("abc-1")).is_ok());
        assert!(checked_removal_path(&state).is_err());
        assert!(checked_removal_path(&outside).is_err());
        assert!(checked_removal_path(&tricky).is_err());

        std::env::remove_var(SUDO_ENV);
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "1");
        let rootless = req("less", image.to_str().unwrap(), "10.8.1.3");
        rt.start(&rootless).await.unwrap();
        let less_bundle = state.join(rootless.id.to_string());
        let less_locked = less_bundle.join("rootfs").join("locked");
        lock_dir(&less_locked);
        let _less_unlock = Unlock(less_locked);
        let err = rt.delete(&rootless.id).await.unwrap_err();
        assert!(err.message.contains("removing"), "{err}");
        assert!(less_bundle.exists());
        assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), "");

        std::env::set_var(SUDO_ENV, "1");
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");
        std::env::set_var("PISTACK_YOUKI_TEST_EUID", "0");
        let _ = std::fs::write(&sudo_log, b"");
        let rooted = req("root", image.to_str().unwrap(), "10.8.1.4");
        rt.start(&rooted).await.unwrap();
        rt.delete(&rooted.id).await.unwrap();
        assert_eq!(std::fs::read_to_string(&sudo_log).unwrap(), "");
    }

    fn install_cleanup_helper(dir: &Path) -> PathBuf {
        let src =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/pistack-youki-cleanup");
        let dest = dir.join("pistack-youki-cleanup");
        std::fs::copy(&src, &dest).unwrap();
        let mut perms = std::fs::metadata(&dest).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).unwrap();
        dest
    }

    fn lock_dir(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("file"), b"x").unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o0);
        std::fs::set_permissions(path, perms).unwrap();
    }

    struct Unlock(PathBuf);
    impl Drop for Unlock {
        fn drop(&mut self) {
            if let Ok(mut perms) = std::fs::metadata(&self.0).map(|meta| meta.permissions()) {
                perms.set_mode(0o755);
                let _ = std::fs::set_permissions(&self.0, perms);
            }
        }
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

    fn temp_dir(label: &str) -> PathBuf {
        let tmp = std::env::temp_dir().join(format!(
            "pistack-youki-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        tmp
    }

    fn plain_rootfs(tmp: &Path) -> MachineRequest {
        let image = tmp.join("rootfs");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("marker"), b"keep").unwrap();
        req("web", image.to_str().unwrap(), "10.4.0.1")
    }

    /// Skip youki's global `--log <file>` so the subcommand is `$1`.
    fn fake_shift_log() -> &'static str {
        r#"
cmd=$1
if [ "$1" = --log ]; then
  cmd=$3
fi
"#
    }

    #[tokio::test]
    async fn create_returns_while_a_child_holds_stdout() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("stdio");
        let _dir = scopeguard_dir(&tmp);
        let log = tmp.join("youki.log");
        let bin = tmp.join("youki");
        let sleeper = tmp.join("sleeper");
        let quoted = shell_quote(&log);
        let sleeper_q = shell_quote(&sleeper);
        let shift = fake_shift_log();
        write_exe(
            &bin,
            &format!(
                r#"#!/bin/sh
printf '%s\n' "$@" >> {quoted}
printf '\n' >> {quoted}
{shift}
if [ "$cmd" = state ]; then
  echo "container $2 does not exist" >&2
  exit 1
fi
if [ "$cmd" = create ]; then
  sleep 60 &
  echo $! > {sleeper_q}
  exit 0
fi
exit 0
"#
            ),
        );
        let state = tmp.join("state");
        let _env = EnvGuard::set(&bin, &state);
        let rt = YoukiRuntime;
        let request = plain_rootfs(&tmp);
        let started = std::time::Instant::now();
        let running = rt.start(&request).await.unwrap();
        let elapsed = started.elapsed();
        if let Ok(text) = std::fs::read_to_string(&sleeper) {
            if let Ok(pid) = text.trim().parse::<i32>() {
                if pid > 0 {
                    // SAFETY: `pid` is the positive pid this test spawned. A
                    // negative or zero pid would signal a process group.
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                }
            }
        }
        assert_eq!(running.power, PowerState::On);
        assert!(
            elapsed < Duration::from_secs(5),
            "create waited for inherited stdout: {elapsed:?}"
        );
        let bundle = state.join(request.id.to_string());
        assert!(
            bundle.join("log").is_file(),
            "container stdio is the bundle log"
        );
        assert!(bundle.join("state.json").is_file());
    }

    #[tokio::test]
    async fn create_timeout_and_failure_delete_force_and_drop_the_bundle() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("cleanup");
        let _dir = scopeguard_dir(&tmp);
        let log = tmp.join("youki.log");
        let bin = tmp.join("youki");
        let quoted = shell_quote(&log);
        let shift = fake_shift_log();
        write_exe(
            &bin,
            &format!(
                r#"#!/bin/sh
printf '%s\n' "$@" >> {quoted}
printf '\n' >> {quoted}
{shift}
if [ "$cmd" = state ]; then
  echo "container $2 does not exist" >&2
  exit 1
fi
if [ "$cmd" = create ]; then
  sleep 30
  exit 0
fi
exit 0
"#
            ),
        );
        let state = tmp.join("state");
        let _env = EnvGuard::set(&bin, &state);
        std::env::set_var(TIMEOUT_ENV, "400");
        let rt = YoukiRuntime;
        let request = plain_rootfs(&tmp);
        let started = std::time::Instant::now();
        let err = rt.start(&request).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("timed out"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timeout took {:?}",
            started.elapsed()
        );
        let lines = arg_lines(&std::fs::read_to_string(&log).unwrap());
        assert!(
            lines
                .windows(2)
                .any(|w| w[0] == "delete" && w[1] == "--force"),
            "{lines:?}"
        );
        assert!(!state.join(request.id.to_string()).exists());

        write_exe(
            &bin,
            &format!(
                r#"#!/bin/sh
printf '%s\n' "$@" >> {quoted}
printf '\n' >> {quoted}
{shift}
if [ "$cmd" = state ]; then
  echo "container $2 does not exist" >&2
  exit 1
fi
if [ "$cmd" = create ]; then
  echo 'youki exploded' >&2
  exit 1
fi
exit 0
"#
            ),
        );
        let before = std::fs::read_to_string(&log).unwrap().len();
        let again = plain_rootfs(&tmp);
        let err = rt.start(&again).await.unwrap_err();
        assert!(err.message.contains("youki exploded"), "{err}");
        let lines = arg_lines(&std::fs::read_to_string(&log).unwrap()[before..]);
        assert!(
            lines
                .windows(2)
                .any(|w| w[0] == "delete" && w[1] == "--force"),
            "{lines:?}"
        );
        assert!(!state.join(again.id.to_string()).exists());
    }

    #[tokio::test]
    async fn create_deletes_a_leftover_container_before_create() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("leftover");
        let _dir = scopeguard_dir(&tmp);
        let log = tmp.join("youki.log");
        let bin = tmp.join("youki");
        write_script(&bin, &log, 0);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&bin, &state);
        let rt = YoukiRuntime;
        let request = plain_rootfs(&tmp);
        let track = tmp.join("youki-ids");
        std::fs::create_dir_all(&track).unwrap();
        std::fs::write(track.join(request.id.to_string()), b"").unwrap();
        rt.start(&request).await.unwrap();
        let lines = arg_lines(&std::fs::read_to_string(&log).unwrap());
        let delete_at = lines
            .windows(2)
            .position(|w| w[0] == "delete" && w[1] == "--force");
        let create_at = lines.iter().position(|line| line == "create");
        assert!(
            delete_at.is_some() && create_at.is_some() && delete_at.unwrap() < create_at.unwrap(),
            "{lines:?}"
        );
        assert!(state
            .join(request.id.to_string())
            .join("state.json")
            .is_file());
    }

    #[tokio::test]
    async fn state_dir_follows_data_dir_and_names_an_unwritable_path() {
        let _guard = env_lock().lock().await;
        let _state = EnvSlot::clear("PISTACK_YOUKI_STATE");
        let _data = EnvSlot::clear("PISTACK_DATA_DIR");
        assert_eq!(state_dir(), PathBuf::from(DEFAULT_STATE));
        let data = PathBuf::from("/tmp/pistack-data-example");
        std::env::set_var("PISTACK_DATA_DIR", &data);
        assert_eq!(state_dir(), data.join("youki"));
        std::env::set_var("PISTACK_YOUKI_STATE", "");
        assert_eq!(state_dir(), data.join("youki"));
        std::env::set_var("PISTACK_YOUKI_STATE", "/var/lib/custom-youki");
        assert_eq!(state_dir(), PathBuf::from("/var/lib/custom-youki"));

        let tmp = temp_dir("statedir");
        let _dir = scopeguard_dir(&tmp);
        let log = tmp.join("youki.log");
        let bin = tmp.join("youki");
        write_script(&bin, &log, 0);
        let blocker = tmp.join("blocker");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let _env = EnvGuard::set(&bin, &blocker);
        let rt = YoukiRuntime;
        let request = plain_rootfs(&tmp);
        let err = rt.start(&request).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(
            err.message.contains(&blocker.display().to_string()),
            "{err}"
        );
        assert!(err.message.contains("PISTACK_YOUKI_STATE"), "{err}");
        assert!(err.message.contains("PISTACK_DATA_DIR"), "{err}");
        assert!(err.message.contains("agent user owns"), "{err}");
        assert!(
            std::fs::read_to_string(&log).unwrap_or_default().is_empty(),
            "a rejected state dir must not call youki"
        );
    }

    fn write_ip(path: &Path, log: &Path) {
        let quoted = shell_quote(log);
        write_exe(
            path,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {quoted}\nprintf '\\n' >> {quoted}\nexit 0\n"
            ),
        );
    }

    fn ip_seq(log: &str, seq: &[&str]) -> bool {
        seq_count(log, seq) > 0
    }

    fn seq_count(log: &str, seq: &[&str]) -> usize {
        let lines: Vec<&str> = log.lines().filter(|line| !line.is_empty()).collect();
        lines
            .windows(seq.len())
            .filter(|window| *window == seq)
            .count()
    }

    fn pod_image(tmp: &Path) -> String {
        let image = tmp.join("image");
        std::fs::create_dir_all(image.join("bin")).unwrap();
        std::fs::write(image.join("marker"), b"keep").unwrap();
        image.to_str().unwrap().to_owned()
    }

    fn lab_request(image: &str) -> MachineRequest {
        let mut request = req("web", "not-a-path", "10.90.0.10");
        request.id = Id::parse(kind::MACHINE, "mach-018f3a2b-1c4d-7e5f-8a6b-0c1d2e3f4a5b").unwrap();
        request.networks = vec!["labnet".into()];
        request.nics = vec![
            crate::runtime::NicAttachment {
                network_id: "net-01a102de-3f33-73bf-86f7-57fe3b47f1ea".into(),
                mac: "52:54:00:AA:BB:CC".into(),
                address: "10.90.0.10".into(),
                prefix: 24,
                gateway: "10.90.0.1".into(),
                routed: true,
                segment_id: None,
                binding: None,
            },
            crate::runtime::NicAttachment {
                network_id: "net-01a102de-3f33-73bf-86f7-57fe3b47f1ea".into(),
                mac: "52:54:00:11:22:33".into(),
                address: "10.90.0.11".into(),
                prefix: 24,
                gateway: String::new(),
                routed: false,
                segment_id: Some(4),
                binding: None,
            },
        ];
        request.user_data = Some(sandbox_doc(image));
        request
    }

    fn read_cfg(bundle: &Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(bundle.join("config.json")).unwrap()).unwrap()
    }

    #[test]
    fn sudoers_sample_names_the_ip_binary() {
        assert!(SUDOERS_SAMPLE.contains("/usr/bin/youki"));
        assert!(SUDOERS_SAMPLE.contains("/usr/local/sbin/pistack-youki-cleanup"));
        assert!(SUDOERS_SAMPLE.contains("/usr/sbin/ip"));
        assert!(SUDOERS_SAMPLE.contains("/usr/bin/ovs-vsctl"));
        assert!(!SUDOERS_SAMPLE.contains("ip link"));
    }

    #[tokio::test]
    async fn pod_joins_its_bridge_and_teardown_removes_the_namespace() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("podnet");
        let _dir = scopeguard_dir(&tmp);
        let youki_log = tmp.join("youki.log");
        let ip_log = tmp.join("ip.log");
        let sudo_log = tmp.join("sudo.log");
        let youki = tmp.join("youki");
        let ip = tmp.join("ip");
        let sudo = tmp.join("sudo");
        write_script(&youki, &youki_log, 0);
        write_ip(&ip, &ip_log);
        write_sudo(&sudo, &sudo_log);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        // A root agent ignores PISTACK_YOUKI_SUDO, including for ip.
        std::env::set_var(SUDO_ENV, "1");
        std::env::set_var(SUDO_BIN_ENV, &sudo);
        std::env::set_var("PISTACK_IP_BIN", &ip);

        let request = lab_request(&pod_image(&tmp));
        let rt = YoukiRuntime;
        let started = rt.start(&request).await.unwrap();
        assert_eq!(
            started.addresses,
            vec!["10.90.0.10".to_string(), "10.90.0.11".to_string()]
        );
        assert!(started.message.is_empty(), "{}", started.message);
        let bundle = state.join(request.id.to_string());
        let cfg = read_cfg(&bundle);
        assert_eq!(
            ns_path(&cfg, "network").as_deref(),
            Some("/var/run/netns/pn018f3a2b1c4d")
        );

        let log = std::fs::read_to_string(&ip_log).unwrap();
        assert!(ip_seq(&log, &["link", "del", "vh525400aabbcc"]), "{log}");
        assert!(ip_seq(&log, &["link", "del", "vp525400aabbcc"]), "{log}");
        assert!(ip_seq(&log, &["link", "del", "vh525400112233"]), "{log}");
        assert!(ip_seq(&log, &["netns", "del", "pn018f3a2b1c4d"]), "{log}");
        assert!(ip_seq(&log, &["netns", "add", "pn018f3a2b1c4d"]), "{log}");
        assert!(
            ip_seq(
                &log,
                &[
                    "link",
                    "add",
                    "vh525400aabbcc",
                    "type",
                    "veth",
                    "peer",
                    "name",
                    "vp525400aabbcc",
                ]
            ),
            "{log}"
        );
        assert!(
            ip_seq(
                &log,
                &["link", "set", "vh525400aabbcc", "master", "pi01a102de3f33"]
            ),
            "{log}"
        );
        assert!(
            ip_seq(&log, &["link", "set", "vh525400aabbcc", "up"]),
            "{log}"
        );
        assert!(
            ip_seq(
                &log,
                &["link", "set", "vh525400112233", "master", "po01a102de3f33"]
            ),
            "{log}"
        );
        assert!(
            ip_seq(
                &log,
                &["link", "set", "vp525400aabbcc", "netns", "pn018f3a2b1c4d",]
            ),
            "{log}"
        );
        assert!(
            ip_seq(
                &log,
                &[
                    "-n",
                    "pn018f3a2b1c4d",
                    "link",
                    "set",
                    "vp525400aabbcc",
                    "name",
                    "eth0",
                ]
            ),
            "{log}"
        );
        assert!(
            ip_seq(
                &log,
                &[
                    "-n",
                    "pn018f3a2b1c4d",
                    "link",
                    "set",
                    "eth0",
                    "address",
                    "52:54:00:aa:bb:cc",
                ]
            ),
            "{log}"
        );
        assert!(
            ip_seq(
                &log,
                &[
                    "-n",
                    "pn018f3a2b1c4d",
                    "addr",
                    "add",
                    "10.90.0.10/24",
                    "dev",
                    "eth0",
                ]
            ),
            "{log}"
        );
        assert!(
            ip_seq(&log, &["-n", "pn018f3a2b1c4d", "link", "set", "lo", "up"]),
            "{log}"
        );
        assert!(
            ip_seq(&log, &["-n", "pn018f3a2b1c4d", "link", "set", "eth0", "up"]),
            "{log}"
        );
        assert!(
            ip_seq(
                &log,
                &[
                    "-n",
                    "pn018f3a2b1c4d",
                    "link",
                    "set",
                    "vp525400112233",
                    "name",
                    "eth1",
                ]
            ),
            "{log}"
        );
        assert!(
            ip_seq(
                &log,
                &[
                    "-n",
                    "pn018f3a2b1c4d",
                    "addr",
                    "add",
                    "10.90.0.11/24",
                    "dev",
                    "eth1",
                ]
            ),
            "{log}"
        );
        assert!(
            ip_seq(
                &log,
                &[
                    "-n",
                    "pn018f3a2b1c4d",
                    "route",
                    "add",
                    "default",
                    "via",
                    "10.90.0.1",
                ]
            ),
            "{log}"
        );
        assert_eq!(seq_count(&log, &["route", "add", "default"]), 1, "{log}");
        assert!(!log.contains("pistack0"), "{log}");
        let sudo_text = std::fs::read_to_string(&sudo_log).unwrap_or_default();
        assert!(sudo_text.is_empty(), "root must not sudo ip: {sudo_text}");

        let again = rt.start(&request).await.unwrap();
        assert_eq!(again.addresses, started.addresses);
        assert_eq!(
            std::fs::read_to_string(&ip_log).unwrap(),
            log,
            "a running pod is not rewired"
        );
        let listed = rt.list().await.unwrap();
        assert_eq!(listed[0].addresses, started.addresses);
        assert!(listed[0].message.is_empty());

        rt.stop(&request.id).await.unwrap();
        let after_stop = std::fs::read_to_string(&ip_log).unwrap();
        assert!(
            seq_count(&after_stop, &["netns", "del", "pn018f3a2b1c4d"]) >= 2,
            "{after_stop}"
        );
        assert!(
            ip_seq(&after_stop, &["link", "del", "vh525400112233"]),
            "{after_stop}"
        );

        // Stop deleted the namespace. The next start must create it again
        // and create the sandbox again so youki joins that namespace.
        let creates_before = count_arg(
            &arg_lines(&std::fs::read_to_string(&youki_log).unwrap()),
            "create",
        );
        let adds_before = seq_count(&after_stop, &["netns", "add", "pn018f3a2b1c4d"]);
        let restarted = rt.start(&request).await.unwrap();
        assert_eq!(restarted.addresses, started.addresses);
        assert!(restarted.message.is_empty(), "{}", restarted.message);
        let cfg = read_cfg(&bundle);
        assert_eq!(
            ns_path(&cfg, "network").as_deref(),
            Some("/var/run/netns/pn018f3a2b1c4d")
        );
        let after_restart = std::fs::read_to_string(&ip_log).unwrap();
        assert!(
            seq_count(&after_restart, &["netns", "add", "pn018f3a2b1c4d"]) > adds_before,
            "{after_restart}"
        );
        let creates_after = count_arg(
            &arg_lines(&std::fs::read_to_string(&youki_log).unwrap()),
            "create",
        );
        assert!(
            creates_after > creates_before,
            "sandbox must be created again ({creates_before} -> {creates_after})"
        );

        rt.delete(&request.id).await.unwrap();
        let after_delete = std::fs::read_to_string(&ip_log).unwrap();
        assert!(
            seq_count(&after_delete, &["netns", "del", "pn018f3a2b1c4d"])
                > seq_count(&log, &["netns", "del", "pn018f3a2b1c4d"]),
            "{after_delete}"
        );
    }

    #[tokio::test]
    async fn failed_create_removes_the_namespace() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("podnet-fail");
        let _dir = scopeguard_dir(&tmp);
        let youki = tmp.join("youki");
        let ip = tmp.join("ip");
        let ip_log = tmp.join("ip.log");
        write_script(&youki, &tmp.join("youki.log"), 1);
        write_ip(&ip, &ip_log);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        std::env::set_var("PISTACK_IP_BIN", &ip);
        let request = lab_request(&pod_image(&tmp));
        let err = YoukiRuntime.start(&request).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        let log = std::fs::read_to_string(&ip_log).unwrap();
        assert!(ip_seq(&log, &["netns", "add", "pn018f3a2b1c4d"]), "{log}");
        assert!(
            seq_count(&log, &["netns", "del", "pn018f3a2b1c4d"]) >= 2,
            "{log}"
        );
        assert!(ip_seq(&log, &["link", "del", "vh525400aabbcc"]), "{log}");
    }

    #[tokio::test]
    async fn sudo_prefixes_ip_when_the_agent_is_not_root() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("podnet-sudo");
        let _dir = scopeguard_dir(&tmp);
        let youki = tmp.join("youki");
        let ip = tmp.join("ip");
        let sudo = tmp.join("sudo");
        let ip_log = tmp.join("ip.log");
        let sudo_log = tmp.join("sudo.log");
        write_script(&youki, &tmp.join("youki.log"), 0);
        write_ip(&ip, &ip_log);
        write_sudo(&sudo, &sudo_log);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        std::env::set_var("PISTACK_YOUKI_TEST_EUID", "1000");
        std::env::set_var("PISTACK_YOUKI_TEST_EGID", "1000");
        std::env::set_var(SUDO_ENV, "1");
        std::env::set_var(SUDO_BIN_ENV, &sudo);
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "0");
        std::env::set_var("PISTACK_IP_BIN", &ip);
        let request = lab_request(&pod_image(&tmp));
        YoukiRuntime.start(&request).await.unwrap();
        let sudo_text = std::fs::read_to_string(&sudo_log).unwrap();
        let ip_path = ip.to_string_lossy().into_owned();
        assert!(
            ip_seq(
                &sudo_text,
                &["-n", &ip_path, "netns", "add", "pn018f3a2b1c4d"]
            ),
            "{sudo_text}"
        );
        assert!(
            ip_seq(
                &sudo_text,
                &[
                    "-n",
                    &ip_path,
                    "link",
                    "set",
                    "vh525400aabbcc",
                    "master",
                    "pi01a102de3f33",
                ]
            ),
            "{sudo_text}"
        );
        let ip_text = std::fs::read_to_string(&ip_log).unwrap();
        assert!(
            ip_seq(
                &ip_text,
                &[
                    "-n",
                    "pn018f3a2b1c4d",
                    "route",
                    "add",
                    "default",
                    "via",
                    "10.90.0.1",
                ]
            ),
            "{ip_text}"
        );
        // `ip -n <netns>` is the namespace selector. The sudo flag is only
        // on the sudo argv, in front of the ip binary.
        assert!(
            !ip_text.contains(&ip_path),
            "ip argv must not include the sudo wrapper: {ip_text}"
        );
    }

    #[tokio::test]
    async fn rootless_agent_reports_that_the_pod_has_no_network() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("podnet-rootless");
        let _dir = scopeguard_dir(&tmp);
        let youki = tmp.join("youki");
        let ip = tmp.join("ip");
        let ip_log = tmp.join("ip.log");
        write_script(&youki, &tmp.join("youki.log"), 0);
        write_ip(&ip, &ip_log);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        std::env::set_var("PISTACK_YOUKI_TEST_EUID", "1000");
        std::env::set_var("PISTACK_YOUKI_TEST_EGID", "1000");
        std::env::set_var("PISTACK_YOUKI_TEST_USERNS", "1");
        std::env::set_var("PISTACK_IP_BIN", &ip);
        let request = lab_request(&pod_image(&tmp));
        let rt = YoukiRuntime;
        let started = rt.start(&request).await.unwrap();
        assert!(started.power == PowerState::On);
        assert!(started.addresses.is_empty(), "{:?}", started.addresses);
        assert!(
            started.message.contains("pod has no network"),
            "{}",
            started.message
        );
        assert!(
            started.message.contains("PISTACK_YOUKI_SUDO"),
            "{}",
            started.message
        );
        let cfg = read_cfg(&state.join(request.id.to_string()));
        assert!(ns_path(&cfg, "network").is_none(), "{cfg}");
        assert!(std::fs::read_to_string(&ip_log)
            .unwrap_or_default()
            .is_empty());
        let again = rt.start(&request).await.unwrap();
        assert!(again.addresses.is_empty());
        assert_eq!(again.message, started.message);
        let listed = rt.list().await.unwrap();
        assert_eq!(listed[0].message, started.message);
        assert!(listed[0].addresses.is_empty());
    }

    #[tokio::test]
    async fn a_bad_mac_is_rejected_before_ip() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("podnet-mac");
        let _dir = scopeguard_dir(&tmp);
        let youki = tmp.join("youki");
        let ip = tmp.join("ip");
        let ip_log = tmp.join("ip.log");
        write_script(&youki, &tmp.join("youki.log"), 0);
        write_ip(&ip, &ip_log);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        std::env::set_var("PISTACK_IP_BIN", &ip);
        let mut request = lab_request(&pod_image(&tmp));
        request.nics[0].mac = "not-a-mac".into();
        let err = YoukiRuntime.start(&request).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest, "{err}");
        assert!(err.message.contains("mac"), "{err}");
        assert!(std::fs::read_to_string(&ip_log)
            .unwrap_or_default()
            .is_empty());
    }

    #[tokio::test]
    async fn explicit_netns_skips_the_veth() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("podnet-env");
        let _dir = scopeguard_dir(&tmp);
        let youki = tmp.join("youki");
        let ip = tmp.join("ip");
        let ip_log = tmp.join("ip.log");
        write_script(&youki, &tmp.join("youki.log"), 0);
        write_ip(&ip, &ip_log);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        let dev = tmp.join("dev.ns");
        std::fs::write(&dev, b"ns").unwrap();
        std::env::set_var(NETNS_ENV, &dev);
        std::env::set_var("PISTACK_IP_BIN", &ip);
        let request = lab_request(&pod_image(&tmp));
        let started = YoukiRuntime.start(&request).await.unwrap();
        assert_eq!(started.addresses[0], "10.90.0.10");
        assert!(started.message.is_empty(), "{}", started.message);
        assert!(std::fs::read_to_string(&ip_log)
            .unwrap_or_default()
            .is_empty());
        let cfg = read_cfg(&state.join(request.id.to_string()));
        assert_eq!(
            ns_path(&cfg, "network").as_deref(),
            Some(dev.to_str().unwrap())
        );
    }

    #[tokio::test]
    async fn a_pod_without_a_port_does_not_call_ip() {
        let _guard = env_lock().lock().await;
        let tmp = temp_dir("podnet-none");
        let _dir = scopeguard_dir(&tmp);
        let youki = tmp.join("youki");
        let ip = tmp.join("ip");
        let ip_log = tmp.join("ip.log");
        write_script(&youki, &tmp.join("youki.log"), 0);
        write_ip(&ip, &ip_log);
        let state = tmp.join("state");
        let _env = EnvGuard::set(&youki, &state);
        std::env::set_var("PISTACK_IP_BIN", &ip);
        let image = pod_image(&tmp);
        let mut request = req("web", "not-a-path", "10.1.0.5");
        request.user_data = Some(sandbox_doc(&image));
        let started = YoukiRuntime.start(&request).await.unwrap();
        assert_eq!(started.addresses, vec!["10.1.0.5".to_string()]);
        assert!(started.message.is_empty());
        let cfg = read_cfg(&state.join(request.id.to_string()));
        assert!(ns_path(&cfg, "network").is_none(), "{cfg}");
        assert!(std::fs::read_to_string(&ip_log)
            .unwrap_or_default()
            .is_empty());
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
