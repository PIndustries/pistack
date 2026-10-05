// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! youki (OCI) container runtime.
//!
//! One bundle per machine under `$PISTACK_YOUKI_STATE` (default
//! `/var/lib/pistack/youki`): `config.json`, a `rootfs`, and a `state.json`
//! this runtime writes after a successful start. The binary is
//! `$PISTACK_YOUKI_BIN` when that is set, otherwise `youki` on `PATH`.
//! A pod image that exists as a mold blob or an OCI directory is unpacked
//! into `rootfs`. The sandbox command is the process recorded in
//! `config.json`. `user_data` and ssh keys are not logged and are not
//! placed on the youki argv. A missing youki binary, an image that cannot
//! be unpacked, or an empty rootfs is unavailable and is not recorded as
//! running.

use super::{MachineRequest, MachineRuntime, PowerState, RunningMachine};
use async_trait::async_trait;
use pistack_types::{kind, ApiError, ApiResult, Id};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
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
        if let Err(e) = write_bundle(&bundle, req).await {
            if prior.is_none() {
                let _ = remove_bundle(&bundle).await;
            }
            return Err(e);
        }
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

#[derive(Debug, Deserialize)]
struct SandboxDoc {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    containers: Vec<SandboxContainer>,
}

#[derive(Debug, Default, Deserialize)]
struct SandboxContainer {
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
}

#[derive(Debug, Default, Deserialize)]
struct SandboxEnv {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    value_from: Option<serde_json::Value>,
}

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
    let raw = req.user_data.as_deref()?;
    let doc: SandboxDoc = serde_json::from_str(raw).ok()?;
    if doc.kind != "PodSandbox" {
        return None;
    }
    let c = doc.containers.into_iter().next().unwrap_or_default();
    let env = c
        .env
        .into_iter()
        .filter(|e| e.value_from.is_none() && !e.name.is_empty())
        .map(|e| (e.name, e.value))
        .collect();
    Some(Launch {
        image: c.image,
        command: c.command,
        args: c.args,
        env,
        cwd: c.working_dir,
    })
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

fn oci_config(
    hostname: &str,
    limits: &Limits,
    args: &[String],
    env: &[String],
    cwd: &str,
) -> serde_json::Value {
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
    if limits.memory_mib > 0 {
        let bytes = limits.memory_mib.saturating_mul(1024 * 1024);
        resources.insert("memory".into(), serde_json::json!({ "limit": bytes }));
    }
    if limits.vcpus > 0 {
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
    serde_json::json!({
        "ociVersion": "1.0.2",
        "hostname": hostname,
        "process": {
            "terminal": false,
            "user": {"uid": 0, "gid": 0},
            "args": args,
            "env": env,
            "cwd": cwd
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
    env_map.insert(
        "PATH".into(),
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
    );
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

async fn write_bundle(bundle: &Path, req: &MachineRequest) -> ApiResult<()> {
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
    tokio::task::spawn_blocking(move || {
        install_bundle(&bundle, &image, &hostname, &limits, launch.as_ref())
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
    let text = serde_json::to_vec_pretty(&oci_config(hostname, limits, &args, &env, &cwd))
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
    loop {
        let Some(ent) = tar.next_entry()? else { break };
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
    use std::io::Write;
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

    fn sandbox_doc(image: &str) -> String {
        serde_json::json!({
            "kind": "PodSandbox",
            "containers": [{
                "image": image,
                "command": ["/usr/bin/web"],
                "args": ["--serve"],
                "working_dir": "/work",
                "env": [
                    {"name": "APP_MODE", "value": "prod"},
                    {"name": "SECRET", "value_from": {"secret": "do-not-log-user-data"}}
                ]
            }]
        })
        .to_string()
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
            buf.extend(std::iter::repeat(0u8).take(pad));
        }
        buf.extend(std::iter::repeat(0u8).take(1024));
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
        assert!(cfg["linux"]["namespaces"].is_array());
        assert_eq!(
            cfg["process"]["args"],
            serde_json::json!(["/usr/bin/web", "--serve"])
        );
        assert_eq!(cfg["process"]["cwd"], "/work");
        let env = cfg["process"]["env"].as_array().unwrap();
        assert!(env.iter().any(|v| v == "APP_MODE=prod"));
        assert!(!bundle.join("image").exists());
        let rootfs_meta = std::fs::symlink_metadata(bundle.join("rootfs")).unwrap();
        assert!(rootfs_meta.file_type().is_dir());
        assert!(!rootfs_meta.file_type().is_symlink());
        assert_eq!(
            std::fs::read_to_string(bundle.join("rootfs").join("marker")).unwrap(),
            "keep"
        );
        let bundle_text = std::fs::read_to_string(bundle.join("config.json")).unwrap()
            + &std::fs::read_to_string(bundle.join("state.json")).unwrap()
            + &tree_text(&bundle.join("rootfs"));
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
        )
        .unwrap_err();
        assert!(err.message.contains("image rootfs is empty"), "{err}");

        match prev_mold {
            Some(v) => std::env::set_var("PISTACK_MOLD_DIR", v),
            None => std::env::remove_var("PISTACK_MOLD_DIR"),
        }
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
