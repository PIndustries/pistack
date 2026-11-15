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

#[path = "agent_ids.rs"]
mod agent_ids;
#[path = "bundle_dir.rs"]
mod bundle_dir;
#[path = "consts.rs"]
mod consts;
pub use consts::EXEC_OUTPUT_CAP;
#[path = "ensure_state_dir.rs"]
mod ensure_state_dir;
#[path = "env_flag.rs"]
mod env_flag;
#[path = "file_uid.rs"]
mod file_uid;
#[path = "is_executable.rs"]
mod is_executable;
#[path = "machine_runtime.rs"]
mod machine_runtime;
#[path = "parse_id.rs"]
mod parse_id;
#[path = "privilege.rs"]
mod privilege;
#[path = "privilege_unavailable.rs"]
mod privilege_unavailable;
#[path = "probe_cgroup_caps.rs"]
mod probe_cgroup_caps;
#[path = "resolve_privilege.rs"]
mod resolve_privilege;
#[path = "resolve_run_mode.rs"]
mod resolve_run_mode;
#[path = "resolve_youki.rs"]
mod resolve_youki;
#[cfg(test)]
#[path = "run_mode_new.rs"]
mod run_mode_new;
#[path = "runtime.rs"]
mod runtime;
pub use runtime::YoukiRuntime;
#[path = "state_dir.rs"]
mod state_dir;
pub(crate) use state_dir::*;
#[path = "state_dir_error.rs"]
mod state_dir_error;
#[path = "sudo_bin.rs"]
mod sudo_bin;
#[path = "sysctl_is_zero.rs"]
mod sysctl_is_zero;
#[path = "unprivileged_userns_available.rs"]
mod unprivileged_userns_available;
#[path = "youki_env_lock.rs"]
mod youki_env_lock;
#[cfg(test)]
pub(crate) use youki_env_lock::*;
mod bundles;
mod cgroups;
mod env;
mod exec;
mod image;
mod lifecycle;
mod network;
mod spec;
mod volumes;
#[path = "youki_exec.rs"]
mod youki_exec;
#[path = "youki_timeout.rs"]
mod youki_timeout;

#[cfg(test)]
pub(in crate::runtime) mod test_support;

#[cfg(test)]
#[path = "state_dir_follows_data_dir_and_names_an_unwritable_path_tests.rs"]
mod state_dir_follows_data_dir_and_names_an_unwritable_path_tests;

#[cfg(test)]
#[path = "sudoers_sample_names_the_ip_binary_tests.rs"]
mod sudoers_sample_names_the_ip_binary_tests;
