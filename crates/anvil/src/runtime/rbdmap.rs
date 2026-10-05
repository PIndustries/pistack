// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Map an RBD image to a host block device for a pod.
//!
//! `youki` calls [`map_image`] when it attaches an RBD volume, stores the
//! device path, and calls [`unmap_image`] or [`unmap_device`] on detach and
//! pod delete. A VM never reaches this module: libvirt attaches the same
//! image as a network disk.
//!
//! `$PISTACK_RBD`, when set and non-empty, replaces the `rbd` binary.
//! Otherwise the command is `rbd` on `PATH`.
//!
//! A root agent runs `rbd` directly. `PISTACK_RBD_SUDO` does not change that.
//! When the agent is not root and `PISTACK_RBD_SUDO` is `1`, `true`, or
//! `yes`, `rbd device map` and `rbd device unmap` run as `sudo -n <rbd> ...`.
//! Listing mapped devices stays direct. `sudo -n` fails closed without a
//! rule. Allow only map and unmap:
//!
//! ```text
//! # /etc/sudoers.d/pistack-rbd
//! ubuntu ALL=(root) NOPASSWD: /usr/bin/rbd device map *, /usr/bin/rbd device unmap *, /usr/bin/rbd map *, /usr/bin/rbd unmap *
//! ```
//!
//! Replace `ubuntu` and `/usr/bin/rbd` with the agent user and the `rbd`
//! path on the host. `$PISTACK_RBD_SUDO_BIN` replaces `sudo` for tests.

use pistack_types::{ApiError, ApiResult};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const TIMEOUT: Duration = Duration::from_secs(30);
const BIN_ENV: &str = "PISTACK_RBD";
const SUDO_ENV: &str = "PISTACK_RBD_SUDO";
const SUDO_BIN_ENV: &str = "PISTACK_RBD_SUDO_BIN";
const SUDOERS_SAMPLE: &str = "ubuntu ALL=(root) NOPASSWD: /usr/bin/rbd device map *, /usr/bin/rbd device unmap *, /usr/bin/rbd map *, /usr/bin/rbd unmap *";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Mapped {
    pool: String,
    namespace: String,
    name: String,
    snap: String,
    device: String,
}

impl Mapped {
    fn is_head(&self) -> bool {
        self.namespace.is_empty() && (self.snap.is_empty() || self.snap == "-")
    }
}

/// Map `pool`/`image` if it is not already mapped. Returns `/dev/rbd/<pool>/<image>`
/// when that link exists, otherwise the device `rbd` reported (`/dev/rbdN`).
pub(crate) async fn map_image(
    pool: &str,
    image: &str,
    monitors: &[String],
    user: &str,
    key_secret: &Option<String>,
    read_only: bool,
) -> ApiResult<String> {
    let spec = normalize(pool, image, monitors, user, key_secret)?;
    let bin = rbd_program();
    let mapped = list_mapped(&bin).await?;
    if let Some(found) = find_head(&mapped, &spec.pool, &spec.image) {
        return published_device(&spec.pool, &spec.image, Some(&found.device));
    }
    let args = map_args(&spec, read_only);
    let out = run(&bin, &args, true).await?;
    let printed = device_from_text(&String::from_utf8_lossy(&out.stdout));
    if let Ok(path) = published_device(&spec.pool, &spec.image, printed.as_deref()) {
        return Ok(path);
    }
    let mapped = list_mapped(&bin).await?;
    if let Some(found) = find_head(&mapped, &spec.pool, &spec.image) {
        return published_device(&spec.pool, &spec.image, Some(&found.device));
    }
    Err(ApiError::unavailable("rbd map did not return a device"))
}

/// Unmap the head image when it is mapped. Already unmapped is success.
pub(crate) async fn unmap_image(pool: &str, image: &str) -> ApiResult<()> {
    ident_ok("pool", pool)?;
    ident_ok("image", image)?;
    let bin = rbd_program();
    let mapped = list_mapped(&bin).await?;
    let Some(found) = find_head(&mapped, pool, image) else {
        return Ok(());
    };
    unmap_listed(&bin, &found.device).await
}

/// Unmap the device recorded at attach. A path that is not mapped is success.
pub(crate) async fn unmap_device(path: &str) -> ApiResult<()> {
    let bin = rbd_program();
    let mapped = list_mapped(&bin).await?;
    let Some(found) = mapped.iter().find(|item| device_matches(item, path)) else {
        return Ok(());
    };
    unmap_listed(&bin, &found.device).await
}

struct Spec {
    pool: String,
    image: String,
    user: String,
    monitors: Vec<String>,
    keyring: Option<PathBuf>,
}

fn normalize(
    pool: &str,
    image: &str,
    monitors: &[String],
    user: &str,
    key_secret: &Option<String>,
) -> ApiResult<Spec> {
    ident_ok("pool", pool)?;
    ident_ok("image", image)?;
    let user = if user.is_empty() { "pistack" } else { user };
    ident_ok("user", user)?;
    if monitors.len() > 16 {
        return Err(ApiError::bad_request("rbd monitor list is too long"));
    }
    let mut normalized = Vec::with_capacity(monitors.len());
    for raw in monitors {
        let (host, port) = parse_monitor(raw)?;
        normalized.push(format!("{host}:{port}"));
    }
    let keyring = keyring_file(key_secret)?;
    Ok(Spec {
        pool: pool.to_owned(),
        image: image.to_owned(),
        user: user.to_owned(),
        monitors: normalized,
        keyring,
    })
}

fn map_args(spec: &Spec, read_only: bool) -> Vec<String> {
    let mut args = vec![
        "device".to_owned(),
        "map".to_owned(),
        format!("{}/{}", spec.pool, spec.image),
        "--id".to_owned(),
        spec.user.clone(),
    ];
    if !spec.monitors.is_empty() {
        args.push("-m".to_owned());
        args.push(spec.monitors.join(","));
    }
    if let Some(keyring) = &spec.keyring {
        args.push("--keyring".to_owned());
        args.push(keyring.to_string_lossy().into_owned());
    }
    if read_only {
        args.push("--read-only".to_owned());
    }
    args
}

fn find_head<'a>(mapped: &'a [Mapped], pool: &str, image: &str) -> Option<&'a Mapped> {
    mapped
        .iter()
        .find(|item| item.is_head() && item.pool == pool && item.name == image)
}

fn device_matches(mapped: &Mapped, path: &str) -> bool {
    if mapped.device == path {
        return true;
    }
    mapped.is_head() && stable_link(&mapped.pool, &mapped.name).to_string_lossy() == path
}

fn published_device(pool: &str, image: &str, mapped: Option<&str>) -> ApiResult<String> {
    let stable = stable_link(pool, image);
    if std::fs::symlink_metadata(&stable).is_ok() {
        return Ok(stable.to_string_lossy().into_owned());
    }
    mapped
        .filter(|path| device_arg_ok(path))
        .map(str::to_owned)
        .ok_or_else(|| ApiError::unavailable("rbd map did not return a device"))
}

fn stable_link(pool: &str, image: &str) -> PathBuf {
    dev_root().join("rbd").join(pool).join(image)
}

fn dev_root() -> PathBuf {
    #[cfg(test)]
    if let Ok(raw) = std::env::var("PISTACK_RBD_DEV_ROOT") {
        let raw = raw.trim();
        let path = Path::new(raw);
        if !raw.is_empty()
            && !raw.contains('\0')
            && path.is_absolute()
            && !path.components().any(|c| matches!(c, Component::ParentDir))
        {
            return PathBuf::from(raw);
        }
    }
    PathBuf::from("/dev")
}

fn device_from_text(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|token| device_arg_ok(token))
        .map(str::to_owned)
}

fn device_arg_ok(path: &str) -> bool {
    let p = Path::new(path);
    path.starts_with("/dev/rbd")
        && !path.contains('\0')
        && !path.chars().any(|c| c.is_whitespace() || c.is_control())
        && p.is_absolute()
        && !p.components().any(|c| matches!(c, Component::ParentDir))
}

async fn unmap_listed(bin: &Path, device: &str) -> ApiResult<()> {
    if !device_arg_ok(device) {
        return Err(ApiError::unavailable(
            "rbd mapped device path cannot be unmapped",
        ));
    }
    let args = vec!["device".to_owned(), "unmap".to_owned(), device.to_owned()];
    run(bin, &args, true).await?;
    Ok(())
}

async fn list_mapped(bin: &Path) -> ApiResult<Vec<Mapped>> {
    const ATTEMPTS: &[&[&str]] = &[
        &["device", "list", "--format", "json"],
        &["showmapped", "--format", "json"],
        &["showmapped"],
    ];
    let mut last = ApiError::unavailable("rbd mapped devices were not listed");
    for args in ATTEMPTS {
        let owned: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        match run(bin, &owned, false).await {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout);
                if let Some(mapped) = parse_mapped(&text) {
                    return Ok(mapped);
                }
                last = ApiError::unavailable(format!(
                    "rbd mapped devices were not listed: {}",
                    output_brief(&out)
                ));
            }
            Err(err) => last = err,
        }
    }
    Err(last)
}

async fn run(bin: &Path, args: &[String], elevate: bool) -> ApiResult<std::process::Output> {
    let program = if elevate && want_sudo() {
        sudo_program()?
    } else {
        bin.to_path_buf()
    };
    let mut cmd = Command::new(&program);
    if elevate && want_sudo() {
        cmd.arg("-n").arg(bin);
    }
    cmd.args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|err| {
        ApiError::unavailable(format!(
            "rbd is not installed ({}: {err}){}",
            program.display(),
            privilege_hint()
        ))
    })?;
    let output = match tokio::time::timeout(TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => {
            return Err(ApiError::unavailable(format!(
                "rbd failed: {err}{}",
                privilege_hint()
            )))
        }
        Err(_) => {
            return Err(ApiError::unavailable(format!(
                "rbd timed out after 30s{}",
                privilege_hint()
            )))
        }
    };
    if output.status.success() {
        return Ok(output);
    }
    let op = args.first().map(String::as_str).unwrap_or("rbd");
    Err(ApiError::unavailable(format!(
        "rbd {op} failed: {}{}",
        output_brief(&output),
        privilege_hint()
    )))
}

fn privilege_hint() -> String {
    if agent_is_root() || env_flag(SUDO_ENV) {
        String::new()
    } else {
        format!(" Set {SUDO_ENV}=1 and allow `{SUDOERS_SAMPLE}`.")
    }
}

fn output_brief(out: &std::process::Output) -> String {
    let err = String::from_utf8_lossy(&out.stderr);
    let err = err.trim();
    if !err.is_empty() {
        return clip(err);
    }
    let code = out.status.code().unwrap_or(-1);
    format!("exit {code}")
}

fn clip(text: &str) -> String {
    const MAX: usize = 300;
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= MAX {
        return flat;
    }
    flat.chars().take(MAX).collect()
}

fn rbd_program() -> PathBuf {
    if let Ok(raw) = std::env::var(BIN_ENV) {
        let raw = raw.trim();
        if !raw.is_empty() && !raw.contains('\0') {
            return PathBuf::from(raw);
        }
    }
    PathBuf::from("rbd")
}

fn want_sudo() -> bool {
    !agent_is_root() && env_flag(SUDO_ENV)
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn agent_is_root() -> bool {
    #[cfg(test)]
    if let Ok(raw) = std::env::var("PISTACK_RBD_TEST_EUID") {
        if let Ok(uid) = raw.parse::<u32>() {
            return uid == 0;
        }
    }
    #[cfg(unix)]
    {
        // geteuid does not fail.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn sudo_program() -> ApiResult<PathBuf> {
    if let Some(raw) = std::env::var_os(SUDO_BIN_ENV) {
        if !raw.is_empty() {
            let path = PathBuf::from(raw);
            if executable(&path) {
                return Ok(path);
            }
            return Err(ApiError::unavailable(format!(
                "rbd map: {SUDO_ENV} is set but sudo is not available. Install sudo and `{SUDOERS_SAMPLE}`"
            )));
        }
    }
    crate::facts::which("sudo").ok_or_else(|| {
        ApiError::unavailable(format!(
            "rbd map: {SUDO_ENV} is set but sudo is not available. Install sudo and `{SUDOERS_SAMPLE}`"
        ))
    })
}

fn executable(path: &Path) -> bool {
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

fn ident_ok(kind: &str, value: &str) -> ApiResult<()> {
    let ok = (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "{kind} must match [A-Za-z0-9._-]{{1,64}}"
        )))
    }
}

fn parse_monitor(raw: &str) -> ApiResult<(String, u16)> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > 128
        || raw.chars().any(|c| c.is_control() || c == '\'' || c == '"')
    {
        return Err(ApiError::bad_request(
            "rbd monitor must be a host or host:port",
        ));
    }
    let (host, port) = if let Some((host, port)) = raw.rsplit_once(':') {
        if host.is_empty() {
            return Err(ApiError::bad_request(
                "rbd monitor must be a host or host:port",
            ));
        }
        let port: u16 = port
            .parse()
            .map_err(|_| ApiError::bad_request("rbd monitor must be a host or host:port"))?;
        if port == 0 {
            return Err(ApiError::bad_request(
                "rbd monitor must be a host or host:port",
            ));
        }
        (host, port)
    } else {
        (raw, 6789)
    };
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(ApiError::bad_request(
            "rbd monitor must be a host or host:port",
        ));
    }
    Ok((host.to_owned(), port))
}

/// `key_secret` is a keyring path. The key bytes are not read and are not
/// placed on a command line. Anything that is not an existing file is refused.
fn keyring_file(secret: &Option<String>) -> ApiResult<Option<PathBuf>> {
    let Some(raw) = secret.as_deref() else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let path = Path::new(raw);
    let safe = path.is_absolute()
        && !raw.contains('\0')
        && !raw.chars().any(|c| c.is_control())
        && !path.components().any(|c| matches!(c, Component::ParentDir));
    if !safe {
        return Err(ApiError::unavailable("rbd keyring file is not available"));
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => Ok(Some(path.to_path_buf())),
        Ok(_) => Err(ApiError::unavailable("rbd keyring file is not available")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Err(ApiError::unavailable("rbd keyring file is not available"))
        }
        Err(_) => Err(ApiError::unavailable("rbd keyring file is not available")),
    }
}

fn parse_mapped(text: &str) -> Option<Vec<Mapped>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some(Vec::new());
    }
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        return parse_mapped_json(trimmed);
    }
    parse_mapped_table(trimmed)
}

fn parse_mapped_json(text: &str) -> Option<Vec<Mapped>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let items = match &value {
        serde_json::Value::Array(items) => items.clone(),
        serde_json::Value::Object(map) => {
            if map.contains_key("device") && map.contains_key("pool") {
                vec![value]
            } else {
                map.values().cloned().collect()
            }
        }
        _ => return None,
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(mapped_item(&item)?);
    }
    Some(out)
}

fn mapped_item(item: &serde_json::Value) -> Option<Mapped> {
    let pool = item.get("pool")?.as_str()?.to_owned();
    let name = item
        .get("name")
        .or_else(|| item.get("image"))?
        .as_str()?
        .to_owned();
    let device = item.get("device")?.as_str()?.to_owned();
    if !device.starts_with("/dev/rbd") {
        return None;
    }
    let namespace = item
        .get("namespace")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_owned();
    let snap = item
        .get("snap")
        .and_then(|value| value.as_str())
        .unwrap_or("-")
        .to_owned();
    Some(Mapped {
        pool,
        namespace,
        name,
        snap,
        device,
    })
}

fn parse_mapped_table(text: &str) -> Option<Vec<Mapped>> {
    let mut out = Vec::new();
    let mut saw = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.first().copied() == Some("id") {
            saw = true;
            continue;
        }
        let device = (*tokens.last()?).to_owned();
        if !device.starts_with("/dev/rbd") {
            return None;
        }
        let item = if tokens.len() == 5 {
            Mapped {
                pool: tokens[1].to_owned(),
                namespace: String::new(),
                name: tokens[2].to_owned(),
                snap: tokens[3].to_owned(),
                device,
            }
        } else if tokens.len() >= 6 {
            Mapped {
                pool: tokens[1].to_owned(),
                namespace: tokens[2].to_owned(),
                name: tokens[3].to_owned(),
                snap: tokens[4].to_owned(),
                device,
            }
        } else {
            return None;
        };
        out.push(item);
        saw = true;
    }
    saw.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_and_table_listings_skip_snapshots() {
        let array = r#"[{"id":"0","pool":"volumes","namespace":"","name":"img-1","snap":"-","device":"/dev/rbd0"},{"id":"1","pool":"volumes","namespace":"","name":"img-1","snap":"snap1","device":"/dev/rbd1"}]"#;
        let mapped = parse_mapped(array).unwrap();
        assert_eq!(
            find_head(&mapped, "volumes", "img-1").unwrap().device,
            "/dev/rbd0"
        );
        let object = r#"{"0":{"pool":"volumes","namespace":"","image":"img-2","snap":"-","device":"/dev/rbd2"}}"#;
        let mapped = parse_mapped(object).unwrap();
        assert_eq!(mapped[0].name, "img-2");
        let table = "\
id pool namespace image snap device
0  volumes            img-1 -    /dev/rbd0
1  volumes ns         img-9 -    /dev/rbd9
2  volumes            img-1 snap /dev/rbd3
";
        let mapped = parse_mapped(table).unwrap();
        assert_eq!(
            find_head(&mapped, "volumes", "img-1").unwrap().device,
            "/dev/rbd0"
        );
        assert!(find_head(&mapped, "volumes", "img-9").is_none());
        assert_eq!(parse_mapped("").unwrap().len(), 0);
        assert_eq!(parse_mapped("[]").unwrap().len(), 0);
    }
}
