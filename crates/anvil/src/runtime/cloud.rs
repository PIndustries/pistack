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

use super::{MachineRequest, MachineRuntime, PowerState, RunningMachine};
use async_trait::async_trait;
use pistack_types::{kind, ApiError, ApiResult, Id};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

const DEFAULT_STATE: &str = "/var/lib/pistack/cloud";
const PROBE: Duration = Duration::from_millis(500);

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
        }
        if req.resources.vcpus == 0 || req.resources.memory_mib == 0 {
            return Err(ApiError::unavailable(
                "cloud-hypervisor runtime: machine resources are not set",
            ));
        }
        let disk = validate_disk(&req.image)?;
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

        let mut cmd = Command::new(&bin);
        cmd.args(&args)
            .kill_on_drop(false)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| {
            ApiError::unavailable(format!(
                "cloud-hypervisor runtime: cannot run {}: {e}",
                bin.display()
            ))
        })?;
        let pid = child.id().ok_or_else(|| {
            ApiError::unavailable("cloud-hypervisor runtime: process did not start")
        })? as i32;
        let stderr = child.stderr.take();
        match tokio::time::timeout(PROBE, child.wait()).await {
            Ok(Ok(status)) => {
                let tail = stderr_tail(&read_stderr(stderr).await);
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
        if saved.power == PowerState::Off && !pid_alive(saved.pid) {
            return Ok(());
        }
        stop_pid(saved.pid).await;
        saved.power = PowerState::Off;
        saved.pid = 0;
        write_state(&dir, &saved).await
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
    match std::env::var("PISTACK_NODE_NAME") {
        Ok(node) if !node.is_empty() && node == name => true,
        _ => false,
    }
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
    }

    impl EnvGuard {
        fn set(bin: &Path, state: &Path) -> Self {
            let prev = Self {
                bin: std::env::var_os("PISTACK_CLOUD_HYPERVISOR_BIN"),
                state: std::env::var_os("PISTACK_CLOUD_STATE"),
                kernel: std::env::var_os("PISTACK_CLOUD_KERNEL"),
                node: std::env::var_os("PISTACK_NODE_NAME"),
            };
            std::env::set_var("PISTACK_CLOUD_HYPERVISOR_BIN", bin);
            std::env::set_var("PISTACK_CLOUD_STATE", state);
            std::env::remove_var("PISTACK_CLOUD_KERNEL");
            prev
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            restore("PISTACK_CLOUD_HYPERVISOR_BIN", self.bin.take());
            restore("PISTACK_CLOUD_STATE", self.state.take());
            restore("PISTACK_CLOUD_KERNEL", self.kernel.take());
            restore("PISTACK_NODE_NAME", self.node.take());
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
                address: "10.9.0.4".into(),
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
        let _env = EnvGuard::set(&bin, &state);

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
        assert!(!text.contains("--net"));
        let lines = arg_lines(&text);
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

        assert_eq!(rt.list().await.unwrap()[0].power, PowerState::On);
        rt.stop(&request.id).await.unwrap();
        assert_eq!(rt.list().await.unwrap()[0].power, PowerState::Off);

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
    }
}
