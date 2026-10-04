// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! youki (OCI) container runtime.
//!
//! One bundle per machine under `$PISTACK_YOUKI_STATE` (default
//! `/var/lib/pistack/youki`): `config.json`, a `rootfs`, and a `state.json`
//! this runtime writes. The binary is `$PISTACK_YOUKI_BIN` when that is set,
//! otherwise `youki` on `PATH`. An absolute image that is a directory is the
//! rootfs and is not copied. Any other image gets an empty rootfs and is
//! recorded in `image`. `state.json` is the source of truth after a
//! successful start.

use super::{MachineRequest, MachineRuntime, PowerState, RunningMachine};
use async_trait::async_trait;
use pistack_types::{kind, ApiError, ApiResult, Id};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_STATE: &str = "/var/lib/pistack/youki";

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
        if let Some(saved) = prior.as_ref() {
            if saved.power == PowerState::On && parse_id(&saved.id)? == id {
                // Already running: do not exec youki again. Addresses come
                // from this request, not from whatever was stored.
                return Ok(RunningMachine {
                    id,
                    addresses: nic_addresses(req),
                    power: PowerState::On,
                });
            }
        }
        // user_data and ssh_keys are not written, logged, or passed to youki.
        write_bundle(&bundle, req).await?;
        let id_str = id.to_string();
        let bundle_str = path_arg(&bundle);
        if prior.is_none() {
            exec_youki(&bin, &["create", "--bundle", &bundle_str, &id_str]).await?;
            if let Err(e) = exec_youki(&bin, &["start", &id_str]).await {
                // Create succeeded and start did not. Drop the container so a
                // retry can create it again, and do not record it as running.
                let _ = exec_youki(&bin, &["delete", &id_str]).await;
                return Err(e);
            }
        } else if let Err(e) = exec_youki(&bin, &["start", &id_str]).await {
            return Err(e);
        }
        let addresses = nic_addresses(req);
        write_state(
            &bundle,
            &Saved {
                id: id_str,
                addresses: addresses.clone(),
                power: PowerState::On,
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
        exec_youki(&bin, &["kill", &id.to_string(), "SIGTERM"]).await?;
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
        let had_state = read_state(&bundle).await?.is_some();
        if let Err(e) = exec_youki(&bin, &["delete", &id.to_string()]).await {
            // A bundle with no state was never created in youki. A container
            // youki has already forgotten is gone too. Anything else stays on
            // disk so a later delete can retry.
            if had_state && !container_missing(&e) {
                return Err(e);
            }
        }
        remove_bundle(&bundle).await
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
        loop {
            let ent = rd.next_entry().await.map_err(|e| {
                ApiError::unavailable(format!("youki runtime: reading {}: {e}", root.display()))
            })?;
            let Some(ent) = ent else { break };
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
}

fn lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
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

/// Absolute directory with no `..` component. Relative paths and anything
/// containing `..` are image names, not roots to follow. Never canonicalised.
fn external_rootfs(image: &str) -> Option<PathBuf> {
    let path = Path::new(image);
    if !path.is_absolute() || has_parent_dir(path) {
        return None;
    }
    if path.is_dir() {
        Some(path.to_path_buf())
    } else {
        None
    }
}

fn has_parent_dir(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
        || path.to_string_lossy().split('/').any(|p| p == "..")
}

fn oci_config(req: &MachineRequest) -> serde_json::Value {
    let mut args: Vec<&str> = req.image.split_whitespace().collect();
    if args.is_empty() {
        args.push("/bin/sh");
    }
    let hostname = if req.name.is_empty() {
        req.id.to_string()
    } else {
        req.name.clone()
    };
    let mut linux = serde_json::json!({
        "namespaces": [
            {"type": "pid"},
            {"type": "network"},
            {"type": "ipc"},
            {"type": "uts"},
            {"type": "mount"}
        ]
    });
    let mut resources = serde_json::Map::new();
    if req.resources.memory_mib > 0 {
        let bytes = req.resources.memory_mib.saturating_mul(1024 * 1024);
        resources.insert("memory".into(), serde_json::json!({ "limit": bytes }));
    }
    if req.resources.vcpus > 0 {
        let period: u64 = 100_000;
        let quota = period.saturating_mul(u64::from(req.resources.vcpus));
        resources.insert(
            "cpu".into(),
            serde_json::json!({ "period": period, "quota": quota }),
        );
    }
    if !resources.is_empty() {
        linux["resources"] = serde_json::Value::Object(resources);
    }
    serde_json::json!({
        "ociVersion": "1.0.2",
        "hostname": hostname,
        "process": {
            "terminal": false,
            "user": {"uid": 0, "gid": 0},
            "args": args,
            "env": ["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"],
            "cwd": "/"
        },
        "root": {"path": "rootfs", "readonly": false},
        "mounts": [
            {"destination": "/proc", "type": "proc", "source": "proc"},
            {
                "destination": "/dev",
                "type": "tmpfs",
                "source": "tmpfs",
                "options": ["nosuid", "strictatime", "mode=755", "size=65536k"]
            },
            {
                "destination": "/sys",
                "type": "sysfs",
                "source": "sysfs",
                "options": ["nosuid", "noexec", "nodev", "ro"]
            }
        ],
        "linux": linux
    })
}

async fn write_bundle(bundle: &Path, req: &MachineRequest) -> ApiResult<()> {
    tokio::fs::create_dir_all(bundle).await.map_err(|e| {
        ApiError::unavailable(format!("youki runtime: creating {}: {e}", bundle.display()))
    })?;
    let external = external_rootfs(&req.image);
    prepare_rootfs(bundle, external.as_deref()).await?;
    let image_path = bundle.join("image");
    if external.is_none() {
        tokio::fs::write(&image_path, req.image.as_bytes())
            .await
            .map_err(|e| {
                ApiError::unavailable(format!(
                    "youki runtime: writing {}: {e}",
                    image_path.display()
                ))
            })?;
    } else if tokio::fs::symlink_metadata(&image_path).await.is_ok() {
        tokio::fs::remove_file(&image_path).await.map_err(|e| {
            ApiError::unavailable(format!(
                "youki runtime: removing {}: {e}",
                image_path.display()
            ))
        })?;
    }
    let cfg = serde_json::to_vec_pretty(&oci_config(req))
        .map_err(|e| ApiError::internal(format!("youki runtime: encoding config: {e}")))?;
    let config_path = bundle.join("config.json");
    tokio::fs::write(&config_path, cfg).await.map_err(|e| {
        ApiError::unavailable(format!(
            "youki runtime: writing {}: {e}",
            config_path.display()
        ))
    })
}

async fn prepare_rootfs(bundle: &Path, external: Option<&Path>) -> ApiResult<()> {
    let rootfs = bundle.join("rootfs");
    clear_rootfs_slot(&rootfs).await?;
    if let Some(src) = external {
        link_rootfs(src, &rootfs).await
    } else {
        tokio::fs::create_dir_all(&rootfs).await.map_err(|e| {
            ApiError::unavailable(format!("youki runtime: creating {}: {e}", rootfs.display()))
        })
    }
}

/// Remove a bundle `rootfs` entry without following a symlink to a root we
/// did not create.
async fn clear_rootfs_slot(rootfs: &Path) -> ApiResult<()> {
    let meta = match tokio::fs::symlink_metadata(rootfs).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(ApiError::unavailable(format!(
                "youki runtime: rootfs {}: {e}",
                rootfs.display()
            )))
        }
    };
    let ft = meta.file_type();
    if ft.is_symlink() || !ft.is_dir() {
        tokio::fs::remove_file(rootfs).await.map_err(|e| {
            ApiError::unavailable(format!("youki runtime: removing {}: {e}", rootfs.display()))
        })
    } else {
        tokio::fs::remove_dir_all(rootfs).await.map_err(|e| {
            ApiError::unavailable(format!("youki runtime: removing {}: {e}", rootfs.display()))
        })
    }
}

async fn link_rootfs(src: &Path, dst: &Path) -> ApiResult<()> {
    #[cfg(unix)]
    {
        tokio::fs::symlink(src, dst).await.map_err(|e| {
            ApiError::unavailable(format!(
                "youki runtime: linking rootfs to {}: {e}",
                src.display()
            ))
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (src, dst);
        Err(ApiError::unavailable(
            "youki runtime: using a directory image requires unix",
        ))
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

async fn exec_youki(bin: &Path, args: &[&str]) -> ApiResult<()> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|e| {
        ApiError::unavailable(format!("youki runtime: cannot run {}: {e}", bin.display()))
    })?;
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
        return Ok(());
    }
    let op = args.first().copied().unwrap_or("youki");
    Err(ApiError::unavailable(format!(
        "youki runtime: {op} failed: {}",
        stderr_tail(&output.stderr, output.status)
    )))
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
    use std::os::unix::fs::PermissionsExt;
    use std::sync::OnceLock;

    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    struct EnvGuard {
        bin: Option<std::ffi::OsString>,
        state: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(bin: &Path, state: &Path) -> Self {
            let prev = Self {
                bin: std::env::var_os("PISTACK_YOUKI_BIN"),
                state: std::env::var_os("PISTACK_YOUKI_STATE"),
            };
            std::env::set_var("PISTACK_YOUKI_BIN", bin);
            std::env::set_var("PISTACK_YOUKI_STATE", state);
            prev
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            restore("PISTACK_YOUKI_BIN", self.bin.take());
            restore("PISTACK_YOUKI_STATE", self.state.take());
        }
    }

    fn restore(key: &str, prev: Option<std::ffi::OsString>) {
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
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
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {quoted}\nprintf '\\n' >> {quoted}\n{stderr}exit {exit_code}\n"
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

        let image = "nginx -g daemon off;";
        let request = req("web", image, "10.1.0.5");
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
        assert!(cfg["linux"]["namespaces"].is_array());
        assert_eq!(
            cfg["process"]["args"],
            serde_json::json!(["nginx", "-g", "daemon", "off;"])
        );
        assert_eq!(
            std::fs::read_to_string(bundle.join("image")).unwrap(),
            image
        );
        let rootfs_meta = std::fs::symlink_metadata(bundle.join("rootfs")).unwrap();
        assert!(rootfs_meta.file_type().is_dir());
        assert!(!rootfs_meta.file_type().is_symlink());
        let bundle_text = std::fs::read_to_string(bundle.join("config.json")).unwrap()
            + &std::fs::read_to_string(bundle.join("state.json")).unwrap()
            + &std::fs::read_to_string(bundle.join("image")).unwrap();
        assert!(!bundle_text.contains("do-not-log-user-data"));
        assert!(!bundle_text.contains("do-not-log-ssh-key"));

        let after_first = std::fs::read_to_string(&log).unwrap();
        assert!(!after_first.contains("do-not-log-user-data"));
        assert!(!after_first.contains("do-not-log-ssh-key"));
        let lines = arg_lines(&after_first);
        assert_eq!(count_arg(&lines, "create"), 1);
        assert_eq!(count_arg(&lines, "start"), 1);
        assert_eq!(count_arg(&lines, "kill"), 0);
        let bundle_arg = bundle.to_string_lossy().into_owned();
        let id_arg = request.id.to_string();
        assert!(lines.windows(4).any(|w| {
            w[0] == "create" && w[1] == "--bundle" && w[2] == bundle_arg && w[3] == id_arg
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
        assert_eq!(count_arg(&after_stop, "kill"), 1);
        assert_eq!(count_arg(&after_stop, "SIGTERM"), 1);
        assert!(after_stop
            .windows(3)
            .any(|w| { w[0] == "kill" && w[1] == request.id.to_string() && w[2] == "SIGTERM" }));
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

        let external = tmp.join("external-rootfs");
        std::fs::create_dir(&external).unwrap();
        std::fs::write(external.join("marker"), "keep").unwrap();
        let ext_req = req("ext", external.to_str().unwrap(), "10.2.0.8");
        rt.start(&ext_req).await.unwrap();
        let ext_bundle = state.join(ext_req.id.to_string());
        let link = std::fs::symlink_metadata(ext_bundle.join("rootfs")).unwrap();
        assert!(link.file_type().is_symlink());
        assert!(!ext_bundle.join("image").exists());
        rt.delete(&ext_req.id).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(external.join("marker")).unwrap(),
            "keep"
        );
        assert!(!ext_bundle.exists());

        let dotted = format!(
            "{}/../{}/external-rootfs",
            tmp.parent().unwrap().display(),
            tmp.file_name().unwrap().to_string_lossy()
        );
        assert!(dotted.contains(".."));
        let dot_req = req("dotted", &dotted, "10.2.0.9");
        rt.start(&dot_req).await.unwrap();
        let dot_bundle = state.join(dot_req.id.to_string());
        let dot_root = std::fs::symlink_metadata(dot_bundle.join("rootfs")).unwrap();
        assert!(dot_root.file_type().is_dir());
        assert!(!dot_root.file_type().is_symlink());
        assert_eq!(
            std::fs::read_to_string(dot_bundle.join("image")).unwrap(),
            dotted
        );
        assert_eq!(
            std::fs::read_to_string(external.join("marker")).unwrap(),
            "keep"
        );
        rt.delete(&dot_req.id).await.unwrap();

        write_script(&bin, &log, 1);
        let bad = req("bad", "nginx", "10.3.0.1");
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
        rt.delete(&request.id).await.unwrap();
        rt.delete(&request.id).await.unwrap();
        assert_eq!(
            rt.stop(&request.id).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert!(rt.list().await.unwrap().is_empty());
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
