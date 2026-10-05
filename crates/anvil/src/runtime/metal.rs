// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Bare-metal runtime.
//!
//! A machine whose name is `PISTACK_NODE_NAME` is this host. `start`
//! records it and `stop` does not power it off. Any other machine is a
//! remote chassis: `start` sets `chassis bootdev pxe`, then runs
//! `ipmitool chassis power on`, when the binary, a BMC address, and a
//! password file are present, and stays unavailable when they are not.
//! The requested image name is stored in metal state for later
//! provisioning work; this runtime does not fetch or write disk images
//! itself. On `delete`, a remote chassis is powered off and, when
//! `$PISTACK_METAL_CLEAN` names an executable, that helper runs with the
//! machine id, BMC address, BMC user, and password file path. With the
//! variable unset, delete does not pretend disks were wiped. The
//! password file is `$PISTACK_IPMI_DIR/<machine-id>` and is never read
//! into a log.

use super::{MachineRequest, MachineRuntime, PowerState, RunningMachine};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};
use std::path::{Path, PathBuf};

const DEFAULT_STATE_DIR: &str = "/var/lib/pistack/metal";

/// This host, recorded as a single machine. There is no hypervisor.
#[derive(Debug, Default)]
pub struct MetalRuntime;

fn node_name() -> ApiResult<String> {
    match std::env::var("PISTACK_NODE_NAME") {
        Ok(v) if !v.trim().is_empty() => Ok(v.trim().to_owned()),
        _ => Err(ApiError::unavailable("this host has no node name")),
    }
}

fn state_dir() -> PathBuf {
    match std::env::var_os("PISTACK_METAL_STATE") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(DEFAULT_STATE_DIR),
    }
}

fn state_name(id: &Id) -> ApiResult<String> {
    let name = format!("{id}.json");
    if name.len() > 96
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(ApiError::bad_request("machine id is not a state file name"));
    }
    Ok(name)
}

fn state_path(id: &Id) -> ApiResult<PathBuf> {
    Ok(state_dir().join(state_name(id)?))
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct MetalRecord {
    machine: RunningMachine,
    local: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bmc_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bmc_user: Option<String>,
    /// Image name from the start request. Empty when the caller did not
    /// ask for one. Recorded so a later provisioner can use it; metal
    /// does not pull the bytes itself.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    image: String,
    /// True after a successful `chassis bootdev pxe` for this chassis.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pxe: bool,
}

fn read_record(path: &Path) -> ApiResult<Option<MetalRecord>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let record = serde_json::from_slice(&bytes)
                .map_err(|e| ApiError::internal(format!("metal state {}: {e}", path.display())))?;
            Ok(Some(record))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ApiError::unavailable(format!(
            "metal state {}: {e}",
            path.display()
        ))),
    }
}

fn read_state(id: &Id) -> ApiResult<Option<MetalRecord>> {
    read_record(&state_path(id)?)
}

fn write_state(record: &MetalRecord) -> ApiResult<()> {
    let path = state_path(&record.machine.id)?;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ApiError::unavailable(format!("metal state dir {}: {e}", parent.display()))
            })?;
        }
    }
    let bytes =
        serde_json::to_vec(record).map_err(|e| ApiError::internal(format!("metal state: {e}")))?;
    std::fs::write(&path, bytes)
        .map_err(|e| ApiError::unavailable(format!("metal state {}: {e}", path.display())))?;
    Ok(())
}

fn forget_state(path: &Path) -> ApiResult<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ApiError::unavailable(format!(
            "metal state {}: {e}",
            path.display()
        ))),
    }
}

fn addresses(req: &MachineRequest) -> Vec<String> {
    req.nics
        .iter()
        .map(|n| n.address.clone())
        .filter(|a| !a.is_empty())
        .collect()
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn ipmitool_bin() -> ApiResult<PathBuf> {
    match std::env::var_os("PISTACK_IPMITOOL") {
        Some(v) if !v.is_empty() => {
            let path = PathBuf::from(v);
            if path.is_file() {
                Ok(path)
            } else {
                Err(ApiError::unavailable("ipmitool is not installed"))
            }
        }
        Some(_) => Err(ApiError::unavailable("ipmitool is not installed")),
        None => which("ipmitool").ok_or_else(|| ApiError::unavailable("ipmitool is not installed")),
    }
}

fn pass_file(id: &Id) -> ApiResult<PathBuf> {
    let dir = match std::env::var_os("PISTACK_IPMI_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            return Err(ApiError::unavailable("bmc password file is missing"));
        }
    };
    let name = id.to_string();
    if name.is_empty()
        || name.len() > 80
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(ApiError::bad_request(
            "machine id is not a password file name",
        ));
    }
    let path = dir.join(name);
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|_| ApiError::unavailable("bmc password file is missing"))?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(ApiError::unavailable("bmc password file is missing"));
    }
    Ok(path)
}

fn bmc_host(raw: &str) -> ApiResult<(String, Option<u16>)> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > 253
        || raw.starts_with('-')
        || raw.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(c, '/' | ';' | '|' | '&' | '$' | '`' | '\\' | '"' | '\'')
        })
    {
        return Err(ApiError::bad_request("bmc address is not a host"));
    }
    if let Some((host, port)) = raw.rsplit_once(':') {
        if !host.is_empty() && !host.contains(':') && port.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(port) = port.parse::<u16>() {
                if port > 0 {
                    return Ok((host.to_owned(), Some(port)));
                }
            }
        }
    }
    Ok((raw.to_owned(), None))
}

fn bmc_user(raw: &str) -> ApiResult<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > 32
        || raw.starts_with('-')
        || !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(ApiError::bad_request("bmc user is not a name"));
    }
    Ok(raw.to_owned())
}

fn clip(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flat.trim();
    if trimmed.len() > 180 {
        format!("{}…", &trimmed[..180])
    } else {
        trimmed.to_owned()
    }
}

async fn ipmi(address: &str, user: &str, pass: &Path, words: &[&str], what: &str) -> ApiResult<()> {
    let bin = ipmitool_bin()?;
    let (host, port) = bmc_host(address)?;
    let user = bmc_user(user)?;
    let pass_s = pass.to_string_lossy().into_owned();
    let mut args = vec![
        "-I".to_owned(),
        "lanplus".into(),
        "-H".into(),
        host,
        "-U".into(),
        user,
        "-f".into(),
        pass_s,
    ];
    if let Some(port) = port {
        args.push("-p".into());
        args.push(port.to_string());
    }
    for w in words {
        args.push((*w).to_owned());
    }
    let output = tokio::process::Command::new(&bin)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ApiError::unavailable("ipmitool is not installed")
            } else {
                ApiError::unavailable(format!("ipmitool failed to start: {e}"))
            }
        })?;
    if output.status.success() {
        return Ok(());
    }
    let detail = clip(&String::from_utf8_lossy(&output.stderr));
    if detail.is_empty() {
        Err(ApiError::unavailable(format!("ipmitool {what} failed")))
    } else {
        Err(ApiError::unavailable(format!(
            "ipmitool {what} failed: {detail}"
        )))
    }
}

async fn chassis(power: &str, address: &str, user: &str, pass: &Path) -> ApiResult<()> {
    ipmi(
        address,
        user,
        pass,
        &["chassis", "power", power],
        &format!("chassis power {power}"),
    )
    .await
}

/// Next boot from the network. Does not install an image; it only points
/// the chassis at PXE so a provisioner can take over.
async fn bootdev_pxe(address: &str, user: &str, pass: &Path) -> ApiResult<()> {
    ipmi(
        address,
        user,
        pass,
        &["chassis", "bootdev", "pxe"],
        "chassis bootdev pxe",
    )
    .await
}

fn clean_bin() -> Option<PathBuf> {
    match std::env::var_os("PISTACK_METAL_CLEAN") {
        Some(v) if !v.is_empty() => {
            let path = PathBuf::from(v);
            if path.is_file() {
                Some(path)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Optional cleaning helper. Absent `$PISTACK_METAL_CLEAN` means delete
/// only powers off; we do not claim disks were wiped.
async fn clean_remote(id: &Id, address: &str, user: &str, pass: &Path) -> ApiResult<()> {
    let Some(bin) = clean_bin() else {
        return Ok(());
    };
    let output = tokio::process::Command::new(&bin)
        .arg(id.to_string())
        .arg(address)
        .arg(user)
        .arg(pass.as_os_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ApiError::unavailable("metal clean helper is missing")
            } else {
                ApiError::unavailable(format!("metal clean helper failed to start: {e}"))
            }
        })?;
    if output.status.success() {
        return Ok(());
    }
    let detail = clip(&String::from_utf8_lossy(&output.stderr));
    if detail.is_empty() {
        Err(ApiError::unavailable("metal clean helper failed"))
    } else {
        Err(ApiError::unavailable(format!(
            "metal clean helper failed: {detail}"
        )))
    }
}

#[async_trait]
impl MachineRuntime for MetalRuntime {
    fn name(&self) -> &'static str {
        "metal"
    }

    async fn start(&self, req: &MachineRequest) -> ApiResult<RunningMachine> {
        let node = node_name()?;
        let running = RunningMachine {
            id: req.id.clone(),
            addresses: addresses(req),
            power: PowerState::On,
        };
        if req.name == node {
            write_state(&MetalRecord {
                machine: running.clone(),
                local: true,
                bmc_address: None,
                bmc_user: None,
                image: req.image.clone(),
                pxe: false,
            })?;
            return Ok(running);
        }
        let Some(address) = req.bmc_address.as_deref().filter(|s| !s.trim().is_empty()) else {
            return Err(ApiError::unprocessable(format!(
                "this host is {node}, not {}",
                req.name
            )));
        };
        let Some(user) = req.bmc_user.as_deref().filter(|s| !s.trim().is_empty()) else {
            return Err(ApiError::bad_request("bmc user is required"));
        };
        ipmitool_bin()?;
        let pass = pass_file(&req.id)?;
        bootdev_pxe(address, user, &pass).await?;
        chassis("on", address, user, &pass).await?;
        write_state(&MetalRecord {
            machine: running.clone(),
            local: false,
            bmc_address: Some(address.to_owned()),
            bmc_user: Some(user.to_owned()),
            image: req.image.clone(),
            pxe: true,
        })?;
        Ok(running)
    }

    async fn stop(&self, id: &Id) -> ApiResult<()> {
        match read_state(id)? {
            Some(record) if record.local => Err(ApiError::unavailable(
                "this host is not powered off by the agent",
            )),
            Some(record) => {
                let address = record
                    .bmc_address
                    .as_deref()
                    .ok_or_else(|| ApiError::unavailable("bmc address is missing"))?;
                let user = record
                    .bmc_user
                    .as_deref()
                    .ok_or_else(|| ApiError::unavailable("bmc user is missing"))?;
                ipmitool_bin()?;
                let pass = pass_file(id)?;
                chassis("off", address, user, &pass).await?;
                let mut record = record;
                record.machine.power = PowerState::Off;
                write_state(&record)?;
                Ok(())
            }
            _ => Err(ApiError::not_found(format!(
                "machine {id} is not on this host"
            ))),
        }
    }

    async fn delete(&self, id: &Id) -> ApiResult<()> {
        let path = state_path(id)?;
        if let Some(record) = read_state(id)? {
            if !record.local {
                if let (Some(address), Some(user)) =
                    (record.bmc_address.as_deref(), record.bmc_user.as_deref())
                {
                    ipmitool_bin()?;
                    let pass = pass_file(id)?;
                    chassis("off", address, user, &pass).await?;
                    clean_remote(id, address, user, &pass).await?;
                }
            }
            forget_state(&path)?;
        }
        Ok(())
    }

    async fn list(&self) -> ApiResult<Vec<RunningMachine>> {
        let dir = state_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(ApiError::unavailable(format!(
                    "metal state {}: {e}",
                    dir.display()
                )));
            }
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| {
                ApiError::unavailable(format!("metal state {}: {e}", dir.display()))
            })?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path).map_err(|e| {
                ApiError::unavailable(format!("metal state {}: {e}", path.display()))
            })?;
            if meta.file_type().is_symlink() || !meta.is_file() {
                continue;
            }
            if let Some(record) = read_record(&path)? {
                out.push(record.machine);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::MetalRuntime;
    use crate::runtime::{
        select, select_runtimes, MachineKind, MachineRequest, MachineRuntime, NicAttachment,
        PowerState,
    };
    use pistack_types::{kind, Id, Resources};
    use std::path::PathBuf;

    struct Var {
        key: &'static str,
        old: Option<String>,
    }

    impl Var {
        fn set(key: &'static str, value: &str) -> Self {
            let old = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, old }
        }
    }

    impl Drop for Var {
        fn drop(&mut self) {
            match &self.old {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn req(name: &str) -> MachineRequest {
        MachineRequest {
            id: Id::new(kind::MACHINE),
            name: name.into(),
            kind: MachineKind::Metal,
            image: "bare".into(),
            resources: Resources {
                vcpus: 1,
                memory_mib: 512,
                disk_gib: 1,
                gpus: 0,
            },
            networks: vec![],
            nics: vec![NicAttachment {
                address: "10.9.0.4".into(),
                ..NicAttachment::default()
            }],
            volumes: vec![],
            user_data: None,
            ssh_keys: vec![],
            overlay: None,
            bmc_address: None,
            bmc_user: None,
        }
    }

    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    #[tokio::test]
    async fn fleet_starts_other_does_not_and_selects_metal() {
        let _guard = env_lock().lock().await;
        let dir: PathBuf = std::env::temp_dir().join(format!(
            "pistack-metal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let _state = Var::set("PISTACK_METAL_STATE", dir.to_str().unwrap());
        let _node = Var::set("PISTACK_NODE_NAME", "fleet");

        std::env::remove_var("PISTACK_NODE_NAME");
        let missing = MetalRuntime.start(&req("fleet")).await.unwrap_err();
        assert_eq!(missing.message, "this host has no node name");
        std::env::set_var("PISTACK_NODE_NAME", "fleet");

        let rt = MetalRuntime;
        let fleet = req("fleet");
        let running = rt.start(&fleet).await.unwrap();
        assert_eq!(running.power, PowerState::On);
        assert_eq!(running.id, fleet.id);
        assert_eq!(running.addresses, vec!["10.9.0.4".to_string()]);

        assert!(rt.start(&req("other")).await.is_err());
        let listed = rt.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, fleet.id);
        assert_eq!(listed[0].power, PowerState::On);

        let stopped = rt.stop(&fleet.id).await.unwrap_err();
        assert_eq!(stopped.message, "this host is not powered off by the agent");
        assert_eq!(rt.list().await.unwrap()[0].power, PowerState::On);

        rt.delete(&fleet.id).await.unwrap();
        assert!(rt.list().await.unwrap().is_empty());
        rt.delete(&fleet.id).await.unwrap();

        let caps = ["metal".to_owned()];
        let selected = select(MachineKind::Metal, &caps, false).unwrap();
        assert_eq!(selected.name(), "metal");
        for cap in ["metal", "ipmi", "redfish"] {
            let caps = [cap.to_owned()];
            assert_eq!(
                select(MachineKind::Metal, &caps, false).unwrap().name(),
                "metal"
            );
            assert_eq!(
                select_runtimes(&caps, false)
                    .get(MachineKind::Metal)
                    .unwrap()
                    .name(),
                "metal"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remote_chassis_uses_ipmitool_and_this_host_does_not() {
        let _guard = env_lock().lock().await;
        let dir = std::env::temp_dir().join(format!(
            "pistack-ipmi-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("state");
        let passes = dir.join("pass");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&passes).unwrap();
        let log = dir.join("ipmi.log");
        let tool = dir.join("ipmitool");
        std::fs::write(
            &tool,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_IPMI_LOG\"\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&tool).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&tool, perms).unwrap();
        }
        let _state = Var::set("PISTACK_METAL_STATE", state.to_str().unwrap());
        let _node = Var::set("PISTACK_NODE_NAME", "fleet");
        let _tool = Var::set("PISTACK_IPMITOOL", tool.to_str().unwrap());
        let _dir = Var::set("PISTACK_IPMI_DIR", passes.to_str().unwrap());
        let _log = Var::set("PISTACK_IPMI_LOG", log.to_str().unwrap());

        let rt = MetalRuntime;
        let mut local = req("fleet");
        local.bmc_address = Some("10.9.1.8".into());
        local.bmc_user = Some("root".into());
        rt.start(&local).await.unwrap();
        assert!(!log.exists(), "this host must not call ipmitool");
        let refused = rt.stop(&local.id).await.unwrap_err();
        assert_eq!(refused.message, "this host is not powered off by the agent");
        assert!(!log.exists());

        let mut remote = req("shelf");
        let missing_tool = rt.start(&remote).await.unwrap_err();
        assert!(
            missing_tool.message.contains("not shelf"),
            "{}",
            missing_tool.message
        );

        remote.bmc_address = Some("10.9.1.8".into());
        remote.bmc_user = Some("root".into());
        std::env::set_var("PISTACK_IPMITOOL", dir.join("no-ipmitool"));
        let missing = rt.start(&remote).await.unwrap_err();
        assert_eq!(missing.message, "ipmitool is not installed");
        std::env::set_var("PISTACK_IPMITOOL", &tool);

        let no_pass = rt.start(&remote).await.unwrap_err();
        assert_eq!(no_pass.message, "bmc password file is missing");
        let pass = passes.join(remote.id.to_string());
        std::fs::write(&pass, "s3cret-do-not-log\n").unwrap();

        let running = rt.start(&remote).await.unwrap();
        assert_eq!(running.power, PowerState::On);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("bootdev"), "{text}");
        assert!(text.contains("pxe"), "{text}");
        assert!(text.contains("chassis"), "{text}");
        assert!(text.contains("power"), "{text}");
        assert!(text.contains("on"), "{text}");
        assert!(text.contains("-H"), "{text}");
        assert!(text.contains("10.9.1.8"), "{text}");
        assert!(text.contains("-U"), "{text}");
        assert!(text.contains("root"), "{text}");
        assert!(text.contains("-f"), "{text}");
        assert!(!text.contains("s3cret-do-not-log"), "{text}");
        let record = super::read_state(&remote.id).unwrap().unwrap();
        assert!(record.pxe);
        assert_eq!(record.image, "bare");

        rt.stop(&remote.id).await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("off"), "{text}");
        let listed = rt.list().await.unwrap();
        assert!(listed
            .iter()
            .any(|m| m.id == local.id && m.power == PowerState::On));
        assert!(listed
            .iter()
            .any(|m| m.id == remote.id && m.power == PowerState::Off));

        rt.delete(&remote.id).await.unwrap();
        rt.delete(&local.id).await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(!text.contains("s3cret-do-not-log"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn remote_delete_runs_clean_helper_when_configured() {
        let _guard = env_lock().lock().await;
        let dir = std::env::temp_dir().join(format!(
            "pistack-metal-clean-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("state");
        let passes = dir.join("pass");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&passes).unwrap();
        let log = dir.join("ipmi.log");
        let clean_log = dir.join("clean.log");
        let tool = dir.join("ipmitool");
        let clean = dir.join("clean.sh");
        std::fs::write(
            &tool,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_IPMI_LOG\"\nexit 0\n",
        )
        .unwrap();
        std::fs::write(
            &clean,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_METAL_CLEAN_LOG\"\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&tool, &clean] {
                let mut perms = std::fs::metadata(path).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(path, perms).unwrap();
            }
        }
        let _state = Var::set("PISTACK_METAL_STATE", state.to_str().unwrap());
        let _node = Var::set("PISTACK_NODE_NAME", "fleet");
        let _tool = Var::set("PISTACK_IPMITOOL", tool.to_str().unwrap());
        let _dir = Var::set("PISTACK_IPMI_DIR", passes.to_str().unwrap());
        let _log = Var::set("PISTACK_IPMI_LOG", log.to_str().unwrap());
        let _clean = Var::set("PISTACK_METAL_CLEAN", clean.to_str().unwrap());
        let _clog = Var::set("PISTACK_METAL_CLEAN_LOG", clean_log.to_str().unwrap());

        let rt = MetalRuntime;
        let mut remote = req("shelf");
        remote.image = "install.iso".into();
        remote.bmc_address = Some("10.9.1.8".into());
        remote.bmc_user = Some("root".into());
        let pass = passes.join(remote.id.to_string());
        std::fs::write(&pass, "s3cret-do-not-log\n").unwrap();
        let running = rt.start(&remote).await.unwrap();
        let record = super::read_state(&running.id).unwrap().unwrap();
        assert_eq!(record.image, "install.iso");
        assert!(record.pxe);

        rt.delete(&running.id).await.unwrap();
        let cleaned = std::fs::read_to_string(&clean_log).unwrap();
        assert!(cleaned.contains(&running.id.to_string()), "{cleaned}");
        assert!(cleaned.contains("10.9.1.8"), "{cleaned}");
        assert!(cleaned.contains("root"), "{cleaned}");
        assert!(cleaned.contains(pass.to_str().unwrap()), "{cleaned}");
        assert!(!cleaned.contains("s3cret-do-not-log"), "{cleaned}");
        assert!(super::read_state(&running.id).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
