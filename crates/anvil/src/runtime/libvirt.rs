// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! libvirt/KVM runtime. The agent shells out to `virsh`, `qemu-img` and
//! `cloud-localds`. There is no libvirt crate linked in, so the binary stays
//! Apache-2.0-clean.
//!
//! One host network, `pistack`, is a NAT network on `10.88.0.0/24` with its
//! own bridge `pistack0`. It is not bridged to the host's physical NIC and it
//! is not OVN. A machine with no NICs lands on that network. A machine with
//! NICs lands on the network named by the port: `10.88.0.0/24` stays
//! `pistack` and is never redefined, and any other network gets its own
//! bridge. Disks live under `/var/lib/libvirt/images/pistack`, which the
//! Ubuntu libvirt AppArmor profile already allows.

use super::{
    MachineRequest, MachineRuntime, NicAttachment, PowerState, RunningMachine, VolumeAttach,
    VolumeDetach,
};
use async_trait::async_trait;
use pistack_rpc::proto::{DiskTarget, PortPlug};
use pistack_types::{kind, ApiError, ApiResult, Id};
use std::net::Ipv4Addr;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

const URI: &str = "qemu:///system";
const NETWORK: &str = "pistack";
const BRIDGE: &str = "pistack0";
const GATEWAY: &str = "10.88.0.1";
const DHCP_START: &str = "10.88.0.10";
const DHCP_END: &str = "10.88.0.250";
const NETMASK: &str = "255.255.255.0";
/// How long start waits for the DHCP lease before returning the machine as on.
const LEASE_WAIT: Duration = Duration::from_secs(45);

/// Host-side KVM runtime.
#[derive(Debug, Default)]
pub struct LibvirtRuntime;

impl LibvirtRuntime {
    fn dir() -> PathBuf {
        std::env::var_os("PISTACK_LIBVIRT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/var/lib/libvirt/images/pistack"))
    }
}

#[async_trait]
impl MachineRuntime for LibvirtRuntime {
    fn name(&self) -> &'static str {
        "libvirt"
    }

    async fn start(&self, req: &MachineRequest) -> ApiResult<RunningMachine> {
        let domain = domain_name(&req.id)?;
        let dir = Self::dir();
        ensure_network().await?;
        match dom_state(&domain).await {
            Ok(state) if state == "running" || state == "paused" => {}
            Ok(_) => {
                virsh(["start", &domain]).await?;
            }
            Err(_) => {
                provision(&dir, &domain, req).await?;
                virsh(["start", &domain]).await?;
            }
        }
        // A port address is what the guest is configured to use. Waiting for
        // a DHCP lease would report a different address, or none, when the
        // seed turns DHCP off.
        let addresses = if req.nics.is_empty() {
            wait_lease(&domain, LEASE_WAIT).await
        } else {
            req.nics
                .iter()
                .map(|n| n.address.clone())
                .filter(|a| !a.is_empty())
                .collect()
        };
        Ok(RunningMachine {
            id: req.id.clone(),
            addresses,
            power: PowerState::On,
        })
    }

    async fn stop(&self, id: &Id) -> ApiResult<()> {
        let domain = domain_name(id)?;
        let state = dom_state(&domain)
            .await
            .map_err(|_| ApiError::not_found(format!("machine {id} is not on this host")))?;
        if state == "shut off" {
            return Ok(());
        }
        let _ = virsh(["shutdown", &domain]).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while tokio::time::Instant::now() < deadline {
            if dom_state(&domain).await.ok().as_deref() == Some("shut off") {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        virsh(["destroy", &domain]).await.map(|_| ())
    }

    async fn delete(&self, id: &Id) -> ApiResult<()> {
        let domain = domain_name(id)?;
        if dom_state(&domain).await.is_ok() {
            let _ = virsh(["destroy", &domain]).await;
            virsh(["undefine", &domain]).await?;
        }
        let vm = Self::dir().join("machines").join(&domain);
        if vm.exists() {
            tokio::fs::remove_dir_all(&vm)
                .await
                .map_err(|e| ApiError::internal(format!("removing {}: {e}", vm.display())))?;
        }
        Ok(())
    }

    async fn attach_volume(&self, req: &VolumeAttach) -> ApiResult<String> {
        let domain = domain_name(&req.machine)?;
        if let Err(e) = dom_state(&domain).await {
            return Err(if e.code == pistack_types::error::ErrorCode::Unavailable {
                e
            } else {
                ApiError::not_found(format!("machine {} is not on this host", req.machine))
            });
        }
        if let Some(dev) = req.device.as_deref() {
            check_device(dev)?;
        }
        let state = dom_state(&domain).await?;
        let live = state == "running" || state == "paused";
        match resolve_disk(req).await? {
            HostDisk::File(source) => {
                let disk = stage_volume(&req.volume, &source).await?;
                attach_host_disk(
                    &domain,
                    &disk.to_string_lossy(),
                    req.device.as_deref(),
                    req.read_only,
                    false,
                    live,
                )
                .await
            }
            HostDisk::Block(source) => {
                attach_host_disk(
                    &domain,
                    &source.to_string_lossy(),
                    req.device.as_deref(),
                    req.read_only,
                    true,
                    live,
                )
                .await
            }
            HostDisk::Rbd(disk) => {
                attach_rbd(&domain, req.device.as_deref(), req.read_only, &disk, live).await
            }
        }
    }

    async fn detach_volume(&self, req: &VolumeDetach) -> ApiResult<()> {
        let domain = domain_name(&req.machine)?;
        if let Err(e) = dom_state(&domain).await {
            return Err(if e.code == pistack_types::error::ErrorCode::Unavailable {
                e
            } else {
                ApiError::not_found(format!("machine {} is not on this host", req.machine))
            });
        }
        if let Some(DiskTarget::Rbd { pool, image, .. }) = &req.target {
            let spec = rbd_names(pool, image)?;
            return detach_by_source(&domain, &spec, req.device.as_deref()).await;
        }
        if let Ok(HostDisk::Block(source)) = classify_path(&req.path).await {
            return detach_by_source(&domain, &source.to_string_lossy(), req.device.as_deref())
                .await;
        }
        let source = PathBuf::from(&req.path);
        let staged = staged_path(&req.volume, &source);
        let listing = virsh(["domblklist", &domain]).await.unwrap_or_default();
        let device = if let Some(dev) = req.device.clone() {
            check_device(&dev)?;
            if target_for_source(&listing, &staged.to_string_lossy()).is_none()
                && !listing.split_whitespace().any(|c| c == dev)
            {
                copy_back(&staged, &source).await?;
                return Ok(());
            }
            dev
        } else if let Some(found) = target_for_source(&listing, &staged.to_string_lossy()) {
            found
        } else {
            copy_back(&staged, &source).await?;
            return Ok(());
        };
        let state = dom_state(&domain).await?;
        let mut args = vec!["detach-disk".to_owned(), domain, device, "--config".into()];
        if state == "running" || state == "paused" {
            args.push("--live".into());
        }
        virsh(args).await?;
        copy_back(&staged, &source).await
    }

    async fn list(&self) -> ApiResult<Vec<RunningMachine>> {
        let out = virsh(["list", "--all", "--name"]).await?;
        let mut machines = Vec::new();
        for line in out.lines() {
            let name = line.trim();
            let Some(id_str) = name.strip_prefix("pistack-") else {
                continue;
            };
            let Some(id) = Id::parse(kind::MACHINE, id_str) else {
                continue;
            };
            let state = dom_state(name).await.unwrap_or_default();
            let power = match state.as_str() {
                "running" | "paused" => PowerState::On,
                "shut off" => PowerState::Off,
                _ => PowerState::Unknown,
            };
            let addresses = if power == PowerState::On {
                leases(name).await.unwrap_or_default()
            } else {
                Vec::new()
            };
            machines.push(RunningMachine {
                id,
                addresses,
                power,
            });
        }
        Ok(machines)
    }
}

async fn provision(root: &Path, domain: &str, req: &MachineRequest) -> ApiResult<()> {
    let base = resolve_image(root, &req.image)?;
    let vm = root.join("machines").join(domain);
    tokio::fs::create_dir_all(&vm)
        .await
        .map_err(|e| ApiError::internal(format!("creating {}: {e}", vm.display())))?;
    let disk = vm.join("disk.qcow2");
    ensure_console_log(&vm.join("console.log")).await?;
    let gib = if req.resources.disk_gib == 0 {
        10
    } else {
        req.resources.disk_gib
    };
    if !disk.exists() {
        let format = image_format(&base).await?;
        let size = format!("{gib}G");
        run(
            "qemu-img",
            [
                "create",
                "-f",
                "qcow2",
                "-F",
                &format,
                "-b",
                &base.to_string_lossy(),
                &disk.to_string_lossy(),
                &size,
            ],
        )
        .await?;
    }
    if let Some(plan) = &req.overlay {
        crate::overlay::apply_plan(plan).await?;
    }
    for nic in &req.nics {
        prepare_nic(nic).await?;
    }
    let user_data = match &req.user_data {
        Some(raw) => raw.clone(),
        None => cloud_config(&req.name, &req.ssh_keys)?,
    };
    let user_path = vm.join("user-data");
    let meta_path = vm.join("meta-data");
    let seed = vm.join("seed.iso");
    tokio::fs::write(&user_path, user_data)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    tokio::fs::write(
        &meta_path,
        format!(
            "instance-id: {domain}\nlocal-hostname: {}\n",
            hostname(&req.name)
        ),
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut seed_args = Vec::new();
    if !req.nics.is_empty() {
        let yaml = network_config(&req.nics)?;
        let net_path = vm.join("network-config");
        tokio::fs::write(&net_path, yaml)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        seed_args.push("--network-config".to_owned());
        seed_args.push(net_path.to_string_lossy().into_owned());
    }
    seed_args.push(seed.to_string_lossy().into_owned());
    seed_args.push(user_path.to_string_lossy().into_owned());
    seed_args.push(meta_path.to_string_lossy().into_owned());
    run("cloud-localds", seed_args.iter().map(String::as_str)).await?;
    let vcpus = req.resources.vcpus.max(1);
    let memory = if req.resources.memory_mib == 0 {
        1024
    } else {
        req.resources.memory_mib
    };
    let xml = domain_xml(domain, memory, vcpus, &disk, &seed, &req.nics)?;
    let xml_path = vm.join("domain.xml");
    tokio::fs::write(&xml_path, xml)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    virsh(["define", &xml_path.to_string_lossy()])
        .await
        .map(|_| ())
}

async fn ensure_network() -> ApiResult<()> {
    if let Ok(info) = virsh(["net-info", NETWORK]).await {
        if !info.lines().any(|l| {
            let l = l.to_ascii_lowercase();
            l.contains("active") && l.contains("yes")
        }) {
            virsh(["net-start", NETWORK]).await?;
        }
        let _ = virsh(["net-autostart", NETWORK]).await;
        return Ok(());
    }
    let xml = network_xml();
    let path = LibvirtRuntime::dir().join("network.xml");
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    tokio::fs::write(&path, xml)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?;
    virsh(["net-define", &path.to_string_lossy()]).await?;
    virsh(["net-start", NETWORK]).await?;
    virsh(["net-autostart", NETWORK]).await.map(|_| ())
}

fn resolve_image(root: &Path, image: &str) -> ApiResult<PathBuf> {
    if image.is_empty() {
        return Err(ApiError::bad_request("image is required"));
    }
    let path = Path::new(image);
    if path.is_absolute() {
        if image.contains('\0') || path.components().any(|c| c.as_os_str() == "..") {
            return Err(ApiError::bad_request("image path is not allowed"));
        }
        return if path.is_file() {
            Ok(path.to_path_buf())
        } else {
            Err(ApiError::not_found(format!("image file {image} not found")))
        };
    }
    if !image_name_ok(image) {
        return Err(ApiError::bad_request(format!(
            "image name {image:?} must be a file name, not a path"
        )));
    }
    let owned: Vec<&str> = match image {
        "ubuntu" | "noble" | "ubuntu-24.04" => vec!["ubuntu-24.04", "ubuntu"],
        other => vec![other],
    };
    for name in &owned {
        for ext in ["qcow2", "img"] {
            let candidate = root.join("bases").join(format!("{name}.{ext}"));
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(ApiError::not_found(format!(
        "no base image {image} under {}/bases (expected ubuntu-24.04.qcow2)",
        root.display()
    )))
}

fn image_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        && !name.contains("..")
}

async fn image_format(path: &Path) -> ApiResult<String> {
    let out = run(
        "qemu-img",
        ["info", "--output=json", &path.to_string_lossy()],
    )
    .await?;
    let v: serde_json::Value = serde_json::from_str(&out)
        .map_err(|e| ApiError::internal(format!("qemu-img info: {e}")))?;
    let format = v.get("format").and_then(|f| f.as_str()).unwrap_or("qcow2");
    if !matches!(format, "qcow2" | "raw") {
        return Err(ApiError::bad_request(format!(
            "image format {format} is not supported"
        )));
    }
    Ok(format.to_owned())
}

async fn wait_lease(domain: &str, budget: Duration) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if let Ok(addrs) = leases(domain).await {
            if !addrs.is_empty() {
                return addrs;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Vec::new();
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn leases(domain: &str) -> ApiResult<Vec<String>> {
    let out = virsh(["domifaddr", domain, "--source", "lease"]).await?;
    Ok(parse_leases(&out))
}

async fn dom_state(domain: &str) -> ApiResult<String> {
    let out = virsh(["domstate", domain]).await?;
    Ok(out.trim().to_owned())
}

/// `PISTACK_VIRSH`, when set and non-empty, replaces the `virsh` binary.
fn virsh_bin() -> String {
    match std::env::var("PISTACK_VIRSH") {
        Ok(v) if !v.trim().is_empty() && !v.contains('\0') => v,
        _ => "virsh".to_owned(),
    }
}

async fn virsh<I, S>(args: I) -> ApiResult<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut full = vec!["-c".to_owned(), URI.to_owned()];
    full.extend(args.into_iter().map(|s| s.as_ref().to_owned()));
    run(&virsh_bin(), full.iter().map(String::as_str)).await
}

async fn run<I, S>(bin: &str, args: I) -> ApiResult<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let out = Command::new(bin)
        .args(args)
        .output()
        .await
        .map_err(|e| ApiError::unavailable(format!("{bin} is not installed ({e})")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let extra = String::from_utf8_lossy(&out.stdout);
        return Err(ApiError::internal(format!(
            "{bin} failed: {}{}",
            err.trim(),
            extra.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `vdb`–`vdz`. `vda` is the guest root disk and is never a data volume.
fn check_device(device: &str) -> ApiResult<()> {
    let b = device.as_bytes();
    if b.len() == 3 && b[0] == b'v' && b[1] == b'd' && (b'b'..=b'z').contains(&b[2]) {
        return Ok(());
    }
    Err(ApiError::bad_request(
        "device must be a virtio name from vdb to vdz",
    ))
}

fn next_device(listing: &str) -> ApiResult<String> {
    for c in b'b'..=b'z' {
        let name = format!("vd{}", c as char);
        let taken = listing.split_whitespace().any(|w| w == name);
        if !taken {
            return Ok(name);
        }
    }
    Err(ApiError::unprocessable("no free virtio disk name"))
}

/// `virsh domblklist` row whose source is `source`.
fn target_for_source(listing: &str, source: &str) -> Option<String> {
    for line in listing.lines() {
        let mut cols = line.split_whitespace();
        let Some(dev) = cols.next() else { continue };
        let Some(src) = cols.next() else { continue };
        if src == source {
            return Some(dev.to_owned());
        }
    }
    None
}

#[cfg(test)]
fn volume_source(path: &str) -> ApiResult<PathBuf> {
    if path.contains('\0') || !Path::new(path).is_absolute() {
        return Err(ApiError::bad_request("volume path must be absolute"));
    }
    let path = PathBuf::from(path);
    if path.components().any(|c| c.as_os_str() == "..") {
        return Err(ApiError::bad_request("volume path is not allowed"));
    }
    if !path.is_file() {
        return Err(ApiError::not_found(format!(
            "volume file {} not found",
            path.display()
        )));
    }
    Ok(path)
}

#[derive(Debug)]
enum HostDisk {
    File(PathBuf),
    Block(PathBuf),
    Rbd(RbdDisk),
}

#[derive(Debug)]
struct RbdDisk {
    pool: String,
    image: String,
    user: String,
    monitors: Vec<(String, u16)>,
    keyring: Option<PathBuf>,
}

fn volume_path(path: &str) -> ApiResult<PathBuf> {
    if path.is_empty()
        || path.contains('\0')
        || path.contains('\n')
        || path.contains('\r')
        || !Path::new(path).is_absolute()
    {
        return Err(ApiError::bad_request("volume path must be absolute"));
    }
    let path = PathBuf::from(path);
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(ApiError::bad_request("volume path is not allowed"));
    }
    Ok(path)
}

/// `/dev/<name>/...`. The path itself must name something inside `/dev`.
fn under_dev(path: &Path) -> bool {
    let mut comps = path.components();
    match (comps.next(), comps.next()) {
        (Some(Component::RootDir), Some(Component::Normal(dev))) => {
            dev == "dev" && comps.next().is_some()
        }
        _ => false,
    }
}

fn classify_flags(
    requested: &Path,
    canon: &Path,
    is_file: bool,
    is_block: bool,
) -> ApiResult<HostDisk> {
    if is_block {
        if !under_dev(requested) || !under_dev(canon) {
            return Err(ApiError::bad_request(
                "block volume must be a device path under /dev",
            ));
        }
        return Ok(HostDisk::Block(canon.to_path_buf()));
    }
    if is_file {
        return Ok(HostDisk::File(requested.to_path_buf()));
    }
    Err(ApiError::bad_request(
        "volume path is not a file or a block device",
    ))
}

async fn classify_path(path: &str) -> ApiResult<HostDisk> {
    let requested = volume_path(path)?;
    let meta = match tokio::fs::symlink_metadata(&requested).await {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::not_found(format!(
                "volume path {} not found",
                requested.display()
            )))
        }
        Err(e) => {
            return Err(ApiError::unavailable(format!(
                "volume path {}: {e}",
                requested.display()
            )))
        }
    };
    // Follow a symlink only after the requested path is known. A symlink
    // outside /dev must not become a block attach of some other device.
    let followed = tokio::fs::metadata(&requested)
        .await
        .map_err(|e| ApiError::unavailable(format!("volume path {}: {e}", requested.display())))?;
    let canon = if meta.file_type().is_symlink() {
        tokio::fs::canonicalize(&requested).await.map_err(|e| {
            ApiError::unavailable(format!("volume path {}: {e}", requested.display()))
        })?
    } else {
        requested.clone()
    };
    use std::os::unix::fs::FileTypeExt;
    let ft = followed.file_type();
    classify_flags(&requested, &canon, ft.is_file(), ft.is_block_device())
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

fn rbd_names(pool: &str, image: &str) -> ApiResult<String> {
    ident_ok("pool", pool)?;
    ident_ok("image", image)?;
    Ok(format!("{pool}/{image}"))
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
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(ApiError::unavailable("rbd keyring file is not available"))
        }
        Err(_) => Err(ApiError::unavailable("rbd keyring file is not available")),
    }
}

fn rbd_disk(
    pool: &str,
    image: &str,
    monitors: &[String],
    user: &str,
    key_secret: &Option<String>,
) -> ApiResult<RbdDisk> {
    ident_ok("pool", pool)?;
    ident_ok("image", image)?;
    let user = if user.is_empty() {
        "pistack".to_owned()
    } else {
        user.to_owned()
    };
    ident_ok("user", &user)?;
    if monitors.len() > 16 {
        return Err(ApiError::bad_request("rbd monitor list is too long"));
    }
    let monitors = monitors
        .iter()
        .map(|m| parse_monitor(m))
        .collect::<ApiResult<Vec<_>>>()?;
    let keyring = keyring_file(key_secret)?;
    Ok(RbdDisk {
        pool: pool.to_owned(),
        image: image.to_owned(),
        user,
        monitors,
        keyring,
    })
}

fn rbd_bin() -> String {
    match std::env::var("PISTACK_RBD_BIN") {
        Ok(v) if !v.trim().is_empty() && !v.contains('\0') => v.trim().to_owned(),
        _ => "rbd".to_owned(),
    }
}

fn rbd_info_args(disk: &RbdDisk) -> Vec<String> {
    let mut args = vec![
        "info".to_owned(),
        format!("{}/{}", disk.pool, disk.image),
        "--id".to_owned(),
        disk.user.clone(),
    ];
    if !disk.monitors.is_empty() {
        args.push("-m".into());
        args.push(
            disk.monitors
                .iter()
                .map(|(host, port)| format!("{host}:{port}"))
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    if let Some(keyring) = &disk.keyring {
        args.push("--keyring".into());
        args.push(keyring.to_string_lossy().into_owned());
    }
    args
}

async fn probe_rbd(disk: &RbdDisk) -> ApiResult<()> {
    let bin = rbd_bin();
    let args = rbd_info_args(disk);
    let child = Command::new(&bin).args(&args).kill_on_drop(true).output();
    match tokio::time::timeout(Duration::from_secs(30), child).await {
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let msg = err.trim();
            Err(ApiError::internal(if msg.is_empty() {
                "rbd info failed".into()
            } else {
                format!("rbd info failed: {msg}")
            }))
        }
        Ok(Err(e)) => Err(ApiError::unavailable(format!("rbd is not installed ({e})"))),
        Err(_) => Err(ApiError::unavailable("rbd info timed out")),
    }
}

async fn resolve_disk(req: &VolumeAttach) -> ApiResult<HostDisk> {
    match &req.target {
        Some(DiskTarget::Rbd {
            pool,
            image,
            monitors,
            user,
            key_secret,
        }) => {
            let disk = rbd_disk(pool, image, monitors, user, key_secret)?;
            probe_rbd(&disk).await?;
            Ok(HostDisk::Rbd(disk))
        }
        Some(DiskTarget::Local { path }) if !path.is_empty() => classify_path(path).await,
        _ => classify_path(&req.path).await,
    }
}

fn attach_disk_args(
    domain: &str,
    source: &str,
    device: &str,
    block: bool,
    read_only: bool,
    live: bool,
) -> Vec<String> {
    let mut args = vec![
        "attach-disk".to_owned(),
        domain.to_owned(),
        source.to_owned(),
        device.to_owned(),
        "--targetbus".into(),
        "virtio".into(),
        "--subdriver".into(),
        "raw".into(),
        "--config".into(),
    ];
    if block {
        args.push("--sourcetype".into());
        args.push("block".into());
    }
    if read_only {
        args.push("--mode".into());
        args.push("readonly".into());
    }
    if live {
        args.push("--live".into());
    }
    args
}

async fn attach_host_disk(
    domain: &str,
    source: &str,
    device: Option<&str>,
    read_only: bool,
    block: bool,
    live: bool,
) -> ApiResult<String> {
    let listing = virsh(["domblklist", domain]).await?;
    if let Some(existing) = target_for_source(&listing, source) {
        return Ok(existing);
    }
    let device = match device {
        Some(dev) => dev.to_owned(),
        None => next_device(&listing)?,
    };
    virsh(attach_disk_args(
        domain, source, &device, block, read_only, live,
    ))
    .await?;
    Ok(device)
}

fn rbd_disk_xml(disk: &RbdDisk, device: &str, read_only: bool) -> String {
    let mut hosts = String::new();
    for (name, port) in &disk.monitors {
        hosts.push_str(&format!(
            "\n    <host name='{}' port='{port}'/>",
            xml_escape(name),
        ));
    }
    let ro = if read_only { "\n  <readonly/>" } else { "" };
    format!(
        "<disk type='network' device='disk'>\n  <driver name='qemu' type='raw'/>\n  <source protocol='rbd' name='{name}'>{hosts}\n  </source>\n  <target dev='{device}' bus='virtio'/>{ro}\n</disk>\n",
        name = xml_escape(&format!("{}/{}", disk.pool, disk.image)),
    )
}

async fn attach_rbd(
    domain: &str,
    device: Option<&str>,
    read_only: bool,
    disk: &RbdDisk,
    live: bool,
) -> ApiResult<String> {
    let listing = virsh(["domblklist", domain]).await?;
    let needle = format!("{}/{}", disk.pool, disk.image);
    if let Some(existing) = target_for_source_containing(&listing, &needle) {
        check_device(&existing)?;
        return Ok(existing);
    }
    let device = match device {
        Some(dev) => dev.to_owned(),
        None => next_device(&listing)?,
    };
    let xml = rbd_disk_xml(disk, &device, read_only);
    attach_device_xml(domain, &xml, live).await?;
    Ok(device)
}

fn target_for_source_containing(listing: &str, needle: &str) -> Option<String> {
    if needle.is_empty() {
        return None;
    }
    for line in listing.lines() {
        let mut cols = line.split_whitespace();
        let Some(dev) = cols.next() else { continue };
        let Some(src) = cols.next() else { continue };
        if src.contains(needle) && check_device(dev).is_ok() {
            return Some(dev.to_owned());
        }
    }
    None
}

async fn detach_by_source(domain: &str, needle: &str, device: Option<&str>) -> ApiResult<()> {
    let listing = virsh(["domblklist", domain]).await.unwrap_or_default();
    let found = target_for_source(&listing, needle)
        .or_else(|| target_for_source_containing(&listing, needle))
        .or_else(|| {
            device
                .filter(|dev| check_device(dev).is_ok())
                .filter(|dev| listing.split_whitespace().any(|col| col == *dev))
                .map(str::to_owned)
        });
    let Some(dev) = found else {
        return Ok(());
    };
    check_device(&dev)?;
    let state = dom_state(domain).await?;
    let mut args = vec![
        "detach-disk".to_owned(),
        domain.to_owned(),
        dev,
        "--config".into(),
    ];
    if state == "running" || state == "paused" {
        args.push("--live".into());
    }
    virsh(args).await.map(|_| ())
}

async fn ensure_console_log(path: &Path) -> ApiResult<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) if meta.file_type().is_file() => Ok(()),
        Ok(_) => Err(ApiError::unavailable(format!(
            "console log {} is not a file",
            path.display()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::write(path, b"")
                .await
                .map_err(|e| ApiError::internal(format!("creating {}: {e}", path.display())))?;
            chmod_rw(path).await
        }
        Err(e) => Err(ApiError::internal(format!(
            "console log {}: {e}",
            path.display()
        ))),
    }
}

fn image_dir() -> PathBuf {
    LibvirtRuntime::dir()
        .canonicalize()
        .unwrap_or_else(|_| LibvirtRuntime::dir())
}

fn staged_path(volume: &Id, source: &Path) -> PathBuf {
    match source.canonicalize() {
        Ok(canon) if canon.starts_with(image_dir()) => canon,
        _ => image_dir().join("volumes").join(format!("{volume}.raw")),
    }
}

/// qemu on Ubuntu can open files under the libvirt image directory. A slab
/// file that lives elsewhere is sparse-copied there. The copy is mode 0666
/// so libvirt-qemu can write it. The original is not chmod'd.
async fn stage_volume(volume: &Id, source: &Path) -> ApiResult<PathBuf> {
    let dest = staged_path(volume, source);
    if dest
        == source
            .canonicalize()
            .unwrap_or_else(|_| source.to_path_buf())
    {
        chmod_rw(&dest).await?;
        return Ok(dest);
    }
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::internal(format!("creating {}: {e}", parent.display())))?;
    }
    run(
        "cp",
        [
            "--sparse=always",
            "--",
            &source.to_string_lossy(),
            &dest.to_string_lossy(),
        ],
    )
    .await?;
    chmod_rw(&dest).await?;
    Ok(dest)
}

async fn copy_back(staged: &Path, source: &Path) -> ApiResult<()> {
    if !staged.is_file() || !source.is_file() {
        return Ok(());
    }
    let Ok(staged_c) = staged.canonicalize() else {
        return Ok(());
    };
    let Ok(source_c) = source.canonicalize() else {
        return Ok(());
    };
    if staged_c == source_c {
        return Ok(());
    }
    run(
        "cp",
        [
            "--sparse=always",
            "--",
            &staged_c.to_string_lossy(),
            &source_c.to_string_lossy(),
        ],
    )
    .await
    .map(|_| ())
}

async fn chmod_rw(path: &Path) -> ApiResult<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))
        .await
        .map_err(|e| ApiError::internal(format!("chmod {}: {e}", path.display())))
}

pub(crate) fn domain_name(id: &Id) -> ApiResult<String> {
    let s = id.to_string();
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(ApiError::bad_request(format!("machine id {s} is not safe")));
    }
    Ok(format!("pistack-{s}"))
}

/// Live-migrate `domain` to `dest_uri`. `PISTACK_VIRSH` selects the binary;
/// otherwise `virsh`. The destination must be a `qemu+ssh://` URI with no
/// whitespace. The guest XML is not redefined.
pub async fn migrate(domain: &str, dest_uri: &str) -> ApiResult<()> {
    if !dest_uri.starts_with("qemu+ssh://") || dest_uri.chars().any(char::is_whitespace) {
        return Err(ApiError::bad_request(
            "migration destination must be qemu+ssh:// without whitespace",
        ));
    }
    let bin =
        std::env::var_os("PISTACK_VIRSH").unwrap_or_else(|| std::ffi::OsString::from("virsh"));
    let shown = bin.to_string_lossy();
    let child = Command::new(&bin)
        .args([
            "migrate",
            "--live",
            "--p2p",
            "--tunnelled",
            domain,
            dest_uri,
        ])
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_secs(60), child).await {
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let extra = String::from_utf8_lossy(&out.stdout);
            Err(ApiError::internal(format!(
                "{shown} failed: {}{}",
                err.trim(),
                extra.trim()
            )))
        }
        Ok(Err(e)) => Err(ApiError::unavailable(format!(
            "{shown} is not installed ({e})"
        ))),
        Err(_) => Err(ApiError::unavailable("virsh migrate timed out")),
    }
}

fn hostname(name: &str) -> String {
    let h: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(63)
        .collect();
    if h.is_empty() {
        "vm".into()
    } else {
        h
    }
}

fn cloud_config(name: &str, keys: &[String]) -> ApiResult<String> {
    let mut out = format!(
        "#cloud-config\nhostname: {}\nmanage_etc_hosts: true\n",
        hostname(name)
    );
    if !keys.is_empty() {
        out.push_str("ssh_authorized_keys:\n");
        for key in keys {
            let one = key.trim();
            if one.is_empty() || one.chars().any(|c| c.is_control()) {
                return Err(ApiError::bad_request("ssh key must be a single line"));
            }
            out.push_str("  - ");
            out.push_str(one);
            out.push('\n');
        }
    }
    Ok(out)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn console_path(disk: &Path) -> PathBuf {
    disk.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.join("console.log"))
        .unwrap_or_else(|| PathBuf::from("console.log"))
}

/// Image directory the agent uses for disks and console logs.
pub fn images_dir() -> PathBuf {
    LibvirtRuntime::dir()
}

fn domain_xml(
    domain: &str,
    memory_mib: u64,
    vcpus: u32,
    disk: &Path,
    seed: &Path,
    nics: &[NicAttachment],
) -> ApiResult<String> {
    let ifaces = interfaces(nics)?;
    let console = console_path(disk);
    Ok(format!(
        r#"<domain type='kvm'>
  <name>{domain}</name>
  <memory unit='MiB'>{memory_mib}</memory>
  <vcpu>{vcpus}</vcpu>
  <os>
    <type arch='x86_64' machine='q35'>hvm</type>
    <boot dev='hd'/>
  </os>
  <features><acpi/><apic/></features>
  <cpu mode='host-passthrough'/>
  <devices>
    <disk type='file' device='disk'>
      <driver name='qemu' type='qcow2'/>
      <source file='{disk}'/>
      <target dev='vda' bus='virtio'/>
    </disk>
    <disk type='file' device='cdrom'>
      <driver name='qemu' type='raw'/>
      <source file='{seed}'/>
      <target dev='sda' bus='sata'/>
      <readonly/>
    </disk>
{ifaces}    <graphics type='vnc' autoport='yes' listen='127.0.0.1'/>
    <serial type='file'>
      <source path='{console}'/>
      <target type='isa-serial' port='0'/>
    </serial>
    <console type='file'>
      <source path='{console}'/>
      <target type='serial' port='0'/>
    </console>
  </devices>
</domain>
"#,
        domain = xml_escape(domain),
        disk = xml_escape(&disk.to_string_lossy()),
        seed = xml_escape(&seed.to_string_lossy()),
        console = xml_escape(&console.to_string_lossy()),
    ))
}

fn interfaces(nics: &[NicAttachment]) -> ApiResult<String> {
    if nics.is_empty() {
        return Ok(format!(
            "    <interface type='network'>\n      <source network='{NETWORK}'/>\n      <model type='virtio'/>\n    </interface>\n"
        ));
    }
    let mut out = String::new();
    for nic in nics {
        if nic.segment_id.is_some() {
            let bridge = overlay_bridge(&nic.network_id)?;
            out.push_str(&format!(
                "    <interface type='bridge'>\n      <source bridge='{bridge}'/>\n      <mac address='{mac}'/>\n      <model type='virtio'/>\n    </interface>\n",
                bridge = xml_escape(&bridge),
                mac = xml_escape(&nic.mac),
            ));
            continue;
        }
        let net = guest_network(nic)?;
        out.push_str(&format!(
            "    <interface type='network'>\n      <source network='{net}'/>\n      <mac address='{mac}'/>\n      <model type='virtio'/>\n    </interface>\n",
            net = xml_escape(&net),
            mac = xml_escape(&nic.mac),
        ));
    }
    Ok(out)
}

enum NicEndpoint {
    Bridge(String),
    Network(String),
}

struct IfaceRow {
    kind: String,
    source: String,
    mac: String,
}

/// Where a hot-plugged port lands. An overlay port (`segment_id` set) uses
/// the `po` bridge from the network id. It never falls through to `pistack0`,
/// even when the address is on the shared subnet.
fn nic_endpoint(nic: &NicAttachment) -> ApiResult<NicEndpoint> {
    check_nic(nic)?;
    if nic.segment_id.is_some() {
        let bridge = overlay_bridge(&nic.network_id)?;
        if bridge == BRIDGE || bridge.starts_with("pi") || !bridge.starts_with("po") {
            return Err(ApiError::bad_request(
                "overlay port cannot plug onto pistack0",
            ));
        }
        return Ok(NicEndpoint::Bridge(bridge));
    }
    Ok(NicEndpoint::Network(guest_network(nic)?))
}

fn endpoint_matches(ep: &NicEndpoint, row: &IfaceRow) -> bool {
    match ep {
        NicEndpoint::Bridge(bridge) => {
            row.kind == "bridge" && row.source == *bridge && row.source != BRIDGE
        }
        NicEndpoint::Network(net) => row.kind == "network" && row.source == *net,
    }
}

fn hotplug_xml(nic: &NicAttachment) -> ApiResult<String> {
    let ep = nic_endpoint(nic)?;
    let xml = interfaces(std::slice::from_ref(nic))?;
    if let NicEndpoint::Bridge(bridge) = &ep {
        if xml.contains("pistack0")
            || xml.contains("network='pistack'")
            || !xml.contains(&format!("bridge='{bridge}'"))
        {
            return Err(ApiError::internal(
                "refusing to plug an overlay port onto pistack0",
            ));
        }
    }
    Ok(xml)
}

fn parse_ifaces(text: &str) -> Vec<IfaceRow> {
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 {
            continue;
        }
        let mac = cols[cols.len() - 1];
        if !mac_ok(mac) {
            continue;
        }
        out.push(IfaceRow {
            kind: cols[1].to_owned(),
            source: cols[2].to_owned(),
            mac: mac.to_owned(),
        });
    }
    out
}

fn iface_by_mac(text: &str, mac: &str) -> Option<IfaceRow> {
    parse_ifaces(text)
        .into_iter()
        .find(|row| row.mac.eq_ignore_ascii_case(mac))
}

fn domain_gone(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("failed to get domain")
        || m.contains("domain not found")
        || m.contains("no domain with matching name")
}

async fn require_domain(domain: &str) -> ApiResult<String> {
    match dom_state(domain).await {
        Ok(state) => Ok(state),
        Err(e) if e.code == pistack_types::error::ErrorCode::Unavailable => Err(e),
        Err(e) if domain_gone(&e.message) => Err(ApiError::unavailable(format!(
            "domain {domain} is not on this host"
        ))),
        Err(e) => Err(e),
    }
}

fn is_live(state: &str) -> bool {
    state == "running" || state == "paused"
}

async fn attach_device_xml(domain: &str, xml: &str, live: bool) -> ApiResult<()> {
    let path = std::env::temp_dir().join(format!(
        "pistack-dev-{}-{}.xml",
        std::process::id(),
        device_nonce()
    ));
    tokio::fs::write(&path, xml)
        .await
        .map_err(|e| ApiError::internal(format!("writing device xml: {e}")))?;
    let mut args = vec![
        "attach-device".to_owned(),
        domain.to_owned(),
        path.to_string_lossy().into_owned(),
        "--config".into(),
    ];
    if live {
        args.push("--live".into());
    }
    let result = virsh(args).await;
    let _ = tokio::fs::remove_file(&path).await;
    result.map(|_| ())
}

fn device_nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

async fn detach_iface(domain: &str, kind: &str, mac: &str, live: bool) -> ApiResult<()> {
    let kind = match kind {
        "bridge" | "network" => kind,
        other => {
            return Err(ApiError::bad_request(format!(
                "nic type {other} is not a port"
            )))
        }
    };
    let mut args = vec![
        "detach-interface".to_owned(),
        domain.to_owned(),
        kind.to_owned(),
        "--mac".to_owned(),
        mac.to_owned(),
        "--config".to_owned(),
    ];
    if live {
        args.push("--live".into());
    }
    virsh(args).await.map(|_| ())
}

/// Attach `req`'s NIC to the libvirt domain. A missing domain is unavailable.
/// An overlay port is plugged onto its `po` bridge, never `pistack0`.
pub async fn plug_port(req: &PortPlug) -> ApiResult<()> {
    let domain = domain_name(&req.machine)?;
    let state = require_domain(&domain).await?;
    let xml = hotplug_xml(&req.nic)?;
    let ep = nic_endpoint(&req.nic)?;
    let listing = virsh(["domiflist", &domain]).await?;
    if let Some(row) = iface_by_mac(&listing, &req.nic.mac) {
        if endpoint_matches(&ep, &row) {
            return Ok(());
        }
        detach_iface(&domain, &row.kind, &req.nic.mac, is_live(&state)).await?;
    }
    if matches!(ep, NicEndpoint::Network(_)) {
        prepare_nic(&req.nic).await?;
    }
    attach_device_xml(&domain, &xml, is_live(&state)).await
}

/// Remove the NIC with this MAC. A missing domain is unavailable. A NIC that
/// is already gone is success.
pub async fn unplug_port(req: &PortPlug) -> ApiResult<()> {
    let domain = domain_name(&req.machine)?;
    let state = require_domain(&domain).await?;
    if !mac_ok(&req.nic.mac) {
        return Err(ApiError::bad_request("nic mac is not an address"));
    }
    let listing = virsh(["domiflist", &domain]).await?;
    let Some(row) = iface_by_mac(&listing, &req.nic.mac) else {
        return Ok(());
    };
    detach_iface(&domain, &row.kind, &req.nic.mac, is_live(&state)).await
}

/// `po` plus 12 hex digits of the network id. Same rule as fabric's overlay
/// names. This is never `pistack0` and never a `pi` host-NAT bridge.
fn overlay_bridge(network_id: &str) -> ApiResult<String> {
    let body = match network_id.split_once('-') {
        Some((kind, rest)) if !rest.is_empty() && kind.chars().all(|c| c.is_ascii_alphabetic()) => {
            rest
        }
        _ => network_id,
    };
    let hex: String = body
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_lowercase())
        .take(12)
        .collect();
    if hex.len() != 12 {
        return Err(ApiError::bad_request(
            "network id does not make an overlay bridge name",
        ));
    }
    let name = format!("po{hex}");
    if name.starts_with("pi") || name == "pistack0" {
        return Err(ApiError::bad_request(format!(
            "bridge {name} is not an overlay bridge"
        )));
    }
    Ok(name)
}

fn network_xml() -> String {
    format!(
        r#"<network>
  <name>{NETWORK}</name>
  <forward mode='nat'/>
  <bridge name='{BRIDGE}' stp='on' delay='0'/>
  <ip address='{GATEWAY}' netmask='{NETMASK}'>
    <dhcp>
      <range start='{DHCP_START}' end='{DHCP_END}'/>
    </dhcp>
  </ip>
</network>
"#
    )
}

/// `pistack` for an address in 10.88.0.0/24. Anything else is `pi` plus the
/// first 12 hex digits of the network id, and that name is also the bridge.
fn guest_network(nic: &NicAttachment) -> ApiResult<String> {
    check_nic(nic)?;
    let ip: Ipv4Addr = nic
        .address
        .parse()
        .map_err(|_| ApiError::bad_request("nic address must be ipv4"))?;
    let oct = ip.octets();
    if oct[0] == 10 && oct[1] == 88 && oct[2] == 0 {
        return Ok(NETWORK.to_owned());
    }
    // `net-01a1...` keeps the `e` in "net" if every hex digit is counted.
    // The kind prefix is alphabetic; the rest is the uuid.
    let body = match nic.network_id.split_once('-') {
        Some((kind, rest)) if !rest.is_empty() && kind.chars().all(|c| c.is_ascii_alphabetic()) => {
            rest
        }
        _ => nic.network_id.as_str(),
    };
    let hex: String = body
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_lowercase())
        .take(12)
        .collect();
    if hex.len() != 12 {
        return Err(ApiError::bad_request(
            "network id does not make a bridge name",
        ));
    }
    let name = format!("pi{hex}");
    if !bridge_name_ok(&name) {
        return Err(ApiError::bad_request(format!(
            "bridge {name} is not allowed"
        )));
    }
    Ok(name)
}

fn bridge_name_ok(name: &str) -> bool {
    let rest = name.strip_prefix("pi").unwrap_or("");
    rest.len() == 12 && rest.chars().all(|c| c.is_ascii_hexdigit()) && name.len() == 14
}

fn bridge_of(net: &str) -> String {
    if net == NETWORK {
        BRIDGE.to_owned()
    } else {
        net.to_owned()
    }
}

fn check_nic(nic: &NicAttachment) -> ApiResult<()> {
    if !mac_ok(&nic.mac) {
        return Err(ApiError::bad_request("nic mac is not an address"));
    }
    if nic.address.parse::<Ipv4Addr>().is_err() {
        return Err(ApiError::bad_request("nic address must be ipv4"));
    }
    if nic.prefix == 0 || nic.prefix > 30 {
        return Err(ApiError::bad_request("nic prefix must be 1-30"));
    }
    if !nic.gateway.is_empty() && nic.gateway.parse::<Ipv4Addr>().is_err() {
        return Err(ApiError::bad_request("nic gateway must be ipv4"));
    }
    if nic.network_id.is_empty()
        || !nic
            .network_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(ApiError::bad_request("nic network id is not safe"));
    }
    Ok(())
}

fn mac_ok(mac: &str) -> bool {
    let parts: Vec<&str> = mac.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

fn netmask(prefix: u8) -> ApiResult<String> {
    if prefix == 0 || prefix > 30 {
        return Err(ApiError::bad_request("prefix must be 1-30"));
    }
    let mask = u32::MAX << (32 - prefix);
    Ok(Ipv4Addr::from(mask).to_string())
}

fn network_config(nics: &[NicAttachment]) -> ApiResult<String> {
    let mut out = String::from("version: 2\nethernets:\n");
    for (i, nic) in nics.iter().enumerate() {
        check_nic(nic)?;
        out.push_str(&format!(
            "  nic{i}:\n    match:\n      macaddress: \"{}\"\n    dhcp4: false\n    addresses:\n      - {}/{}\n",
            nic.mac, nic.address, nic.prefix
        ));
        if nic.routed && !nic.gateway.is_empty() {
            out.push_str(&format!(
                "    gateway4: {}\n    routes:\n      - to: default\n        via: {}\n",
                nic.gateway, nic.gateway
            ));
        }
    }
    Ok(out)
}

fn project_network_xml(name: &str, nic: &NicAttachment) -> ApiResult<String> {
    check_nic(nic)?;
    if !bridge_name_ok(name) {
        return Err(ApiError::bad_request(format!(
            "bridge {name} is not allowed"
        )));
    }
    if nic.gateway.is_empty() {
        return Err(ApiError::bad_request("network needs a gateway address"));
    }
    let mask = netmask(nic.prefix)?;
    let forward = if nic.routed {
        "<forward mode='nat'/>\n  "
    } else {
        ""
    };
    Ok(format!(
        r#"<network>
  <name>{name}</name>
  {forward}<bridge name='{name}' stp='on' delay='0'/>
  <ip address='{gw}' netmask='{mask}'>
    <dhcp>
      <host mac='{mac}' ip='{ip}'/>
    </dhcp>
  </ip>
</network>
"#,
        gw = xml_escape(&nic.gateway),
        mac = xml_escape(&nic.mac),
        ip = xml_escape(&nic.address),
    ))
}

async fn prepare_nic(nic: &NicAttachment) -> ApiResult<()> {
    if nic.segment_id.is_some() {
        // The overlay helper owns this bridge. Do not define a NAT network
        // and do not move an address onto it from here.
        overlay_bridge(&nic.network_id)?;
        return Ok(());
    }
    let name = guest_network(nic)?;
    let bridge = bridge_of(&name);
    if name == NETWORK {
        ensure_network().await?;
        add_dhcp_host(&name, &nic.mac, &nic.address).await?;
    } else if virsh(["net-info", &name]).await.is_err() {
        let xml = project_network_xml(&name, nic)?;
        let path = LibvirtRuntime::dir().join(format!("{name}.xml"));
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?;
        }
        tokio::fs::write(&path, xml)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        virsh(["net-define", &path.to_string_lossy()]).await?;
        virsh(["net-start", &name]).await?;
        let _ = virsh(["net-autostart", &name]).await;
    } else {
        if !network_active(&name).await? {
            virsh(["net-start", &name]).await?;
        }
        let _ = virsh(["net-autostart", &name]).await;
        add_dhcp_host(&name, &nic.mac, &nic.address).await?;
    }
    metadata_on(&bridge).await;
    Ok(())
}

async fn network_active(name: &str) -> ApiResult<bool> {
    let info = virsh(["net-info", name]).await?;
    Ok(info.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.contains("active") && l.contains("yes")
    }))
}

async fn add_dhcp_host(name: &str, mac: &str, ip: &str) -> ApiResult<()> {
    let xml = virsh(["net-dumpxml", name]).await?;
    if xml.to_ascii_lowercase().contains(&mac.to_ascii_lowercase()) {
        return Ok(());
    }
    let host = format!("<host mac='{mac}' ip='{ip}'/>");
    match virsh([
        "net-update",
        name,
        "add",
        "ip-dhcp-host",
        host.as_str(),
        "--live",
        "--config",
    ])
    .await
    {
        Ok(_) => Ok(()),
        Err(e) => {
            let msg = e.message.to_ascii_lowercase();
            if msg.contains("exist") || msg.contains("already") {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

/// Point the bridge at the local metadata port. A missing helper is a
/// warning: the guest still boots, and metadata answers once the DNAT
/// is installed.
async fn metadata_on(bridge: &str) {
    if bridge != BRIDGE && !bridge_name_ok(bridge) {
        tracing::warn!(bridge, "metadata bridge name refused");
        return;
    }
    let bin = std::env::var("PISTACK_FIP_BIN")
        .unwrap_or_else(|_| "/usr/local/sbin/pistack-fip".to_owned());
    if std::env::var_os("PISTACK_FIP_BIN").is_none() && !Path::new(&bin).exists() {
        tracing::warn!(
            bridge,
            "pistack-fip is missing; metadata redirect not installed"
        );
        return;
    }
    let run = Command::new("sudo")
        .args(["-n", &bin, "metadata-on", "--bridge", bridge])
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_secs(5), run).await {
        Ok(Ok(out)) if out.status.success() => {}
        Ok(Ok(out)) => {
            let err = String::from_utf8_lossy(&out.stderr);
            tracing::warn!(bridge, error = %err.trim(), "metadata redirect was not installed");
        }
        Ok(Err(e)) => tracing::warn!(bridge, error = %e, "metadata redirect was not installed"),
        Err(_) => tracing::warn!(bridge, "metadata redirect timed out"),
    }
}

fn parse_leases(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut cols = line.split_whitespace();
        let Some(proto) = cols.nth(2) else { continue };
        if !proto.eq_ignore_ascii_case("ipv4") && !proto.eq_ignore_ascii_case("ipv6") {
            continue;
        }
        if let Some(addr) = cols.next() {
            let ip = addr.split('/').next().unwrap_or(addr);
            if !ip.is_empty() {
                out.push(ip.to_owned());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_types::Resources;

    fn req(image: &str) -> MachineRequest {
        MachineRequest {
            id: Id::parse(kind::MACHINE, "mach-018f2c3a-7b6e-7a11-8c22-123456789abc").unwrap(),
            name: "Web Tier".into(),
            kind: super::super::MachineKind::Vm,
            image: image.into(),
            resources: Resources {
                vcpus: 1,
                memory_mib: 1024,
                disk_gib: 10,
                gpus: 0,
            },
            networks: vec![],
            nics: vec![],
            volumes: vec![],
            user_data: None,
            ssh_keys: vec!["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestKey demo".into()],
            overlay: None,
            bmc_address: None,
            bmc_user: None,
        }
    }

    #[test]
    fn domain_xml_points_at_the_nat_network_and_escapes() {
        let xml = domain_xml(
            "pistack-mach-1",
            1024,
            1,
            Path::new("/var/lib/libvirt/images/pistack/a&b.qcow2"),
            Path::new("/var/lib/libvirt/images/pistack/seed.iso"),
            &[],
        )
        .unwrap();
        assert!(xml.contains("<source network='pistack'/>"));
        assert!(xml.contains("listen='127.0.0.1'"));
        assert!(xml.contains("a&amp;b.qcow2"));
        assert!(!xml.contains("a&b"));
        assert!(xml.contains("<memory unit='MiB'>1024</memory>"));
        assert!(xml.contains("console.log"));
        assert!(xml.contains("<console type='file'>"));
        assert!(!xml.contains("type='pty'"));
    }

    #[test]
    fn network_xml_is_nat_on_an_isolated_subnet() {
        let xml = network_xml();
        assert!(xml.contains("<forward mode='nat'/>"));
        assert!(xml.contains("10.88.0.1"));
        assert!(xml.contains("pistack0"));
        assert!(!xml.contains("enp"));
    }

    #[test]
    fn leases_parser_reads_ipv4() {
        let text = "\
 Name       MAC address          Protocol     Address
-------------------------------------------------------------------------------
 vnet0      52:54:00:11:22:33    ipv4         10.88.0.12/24
";
        assert_eq!(parse_leases(text), vec!["10.88.0.12".to_owned()]);
    }

    #[test]
    fn cloud_config_embeds_the_key_and_rejects_newlines() {
        let cfg = cloud_config("Web Tier", &["ssh-ed25519 AAAAkey demo".into()]).unwrap();
        assert!(cfg.starts_with("#cloud-config\n"));
        assert!(cfg.contains("hostname: web-tier"));
        assert!(cfg.contains("ssh-ed25519 AAAAkey demo"));
        let bad = cloud_config("n", &["ssh-ed25519 AAAA\ninjected".into()]);
        assert!(bad.is_err());
    }

    #[test]
    fn image_names_reject_paths() {
        let dir = std::env::temp_dir().join(format!("pistack-img-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bases")).unwrap();
        std::fs::write(
            dir.join("bases").join("ubuntu-24.04.qcow2"),
            b"not-a-real-image",
        )
        .unwrap();
        let found = resolve_image(&dir, "ubuntu").unwrap();
        assert!(found.ends_with("ubuntu-24.04.qcow2"));
        assert!(resolve_image(&dir, "../etc/passwd").is_err());
        assert!(resolve_image(&dir, "nope").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_disks_are_vdb_through_vdz() {
        assert!(check_device("vdb").is_ok());
        assert!(check_device("vdz").is_ok());
        assert!(check_device("vda").is_err());
        assert!(check_device("sda").is_err());
        assert!(check_device("vdb1").is_err());
    }

    #[test]
    fn next_device_skips_disks_already_listed() {
        let listing = "\
 Target   Source
------------------------
 vda      /images/disk.qcow2
 vdb      /images/data.raw
";
        assert_eq!(next_device(listing).unwrap(), "vdc");
        assert_eq!(
            target_for_source(listing, "/images/data.raw").as_deref(),
            Some("vdb")
        );
    }

    #[test]
    fn volume_source_rejects_relative_and_dotdot() {
        let file = std::env::temp_dir().join(format!("pistack-vol-{}", std::process::id()));
        std::fs::write(&file, b"disk").unwrap();
        let abs = file.to_string_lossy().into_owned();
        assert!(volume_source(&abs).is_ok());
        assert!(volume_source("relative.raw").is_err());
        assert!(volume_source("/tmp/../etc/passwd").is_err());
        let _ = std::fs::remove_file(&file);
    }

    fn nic(address: &str, routed: bool) -> NicAttachment {
        NicAttachment {
            network_id: "net-01a102de-3f33-73bf-86f7-57fe3b47f1ea".into(),
            mac: "52:54:00:aa:bb:cc".into(),
            address: address.into(),
            prefix: 24,
            gateway: "10.90.0.1".into(),
            routed,
            segment_id: None,
        }
    }

    #[test]
    fn a_shared_address_stays_on_pistack_and_a_project_address_gets_its_own_bridge() {
        let shared = nic("10.88.0.10", true);
        let xml = domain_xml(
            "pistack-mach-1",
            512,
            1,
            Path::new("/disk.qcow2"),
            Path::new("/seed.iso"),
            std::slice::from_ref(&shared),
        )
        .unwrap();
        assert!(xml.contains("<source network='pistack'/>"));
        assert!(xml.contains("52:54:00:aa:bb:cc"));
        assert!(xml.contains("listen='127.0.0.1'"));

        let project = nic("10.90.0.10", true);
        assert_eq!(guest_network(&project).unwrap(), "pi01a102de3f33");
        let xml = domain_xml(
            "pistack-mach-1",
            512,
            1,
            Path::new("/disk.qcow2"),
            Path::new("/seed.iso"),
            std::slice::from_ref(&project),
        )
        .unwrap();
        assert!(xml.contains("<source network='pi01a102de3f33'/>"));
        assert!(!xml.contains("network='pistack'"));

        let nat = project_network_xml("pi01a102de3f33", &project).unwrap();
        assert!(nat.contains("<forward mode='nat'/>"));
        assert!(nat.contains("<host mac='52:54:00:aa:bb:cc' ip='10.90.0.10'/>"));
        assert!(!nat.contains("<range"));
        let mut overlay = nic("10.90.0.10", true);
        overlay.segment_id = Some(7);
        let xml = domain_xml(
            "pistack-mach-1",
            512,
            1,
            Path::new("/disk.qcow2"),
            Path::new("/seed.iso"),
            std::slice::from_ref(&overlay),
        )
        .unwrap();
        assert!(xml.contains("<interface type='bridge'>"));
        assert!(xml.contains("<source bridge='po01a102de3f33'/>"));
        assert!(!xml.contains("forward"));
        assert!(!xml.contains("pistack0"));
        assert_eq!(
            overlay_bridge(&overlay.network_id).unwrap(),
            "po01a102de3f33"
        );

        let isolated = nic("10.90.0.10", false);
        let quiet = project_network_xml("pi01a102de3f33", &isolated).unwrap();
        assert!(!quiet.contains("forward"));

        let plan = network_config(&[project.clone(), isolated]).unwrap();
        assert!(plan.contains("dhcp4: false"));
        assert!(plan.contains("10.90.0.10/24"));
        assert!(plan.contains("gateway4: 10.90.0.1"));
        let only = network_config(std::slice::from_ref(&nic("10.90.0.10", false))).unwrap();
        assert!(!only.contains("gateway4"));
        let mut bad = nic("10.90.0.10", false);
        bad.mac = "not-a-mac".into();
        assert!(network_config(std::slice::from_ref(&bad)).is_err());
        bad.mac = "52:54:00:aa:bb:cc".into();
        bad.network_id = "short".into();
        assert!(guest_network(&bad).is_err());
    }

    #[test]
    fn domain_name_uses_the_machine_id() {
        let r = req("ubuntu");
        assert_eq!(
            domain_name(&r.id).unwrap(),
            "pistack-mach-018f2c3a-7b6e-7a11-8c22-123456789abc"
        );
    }

    static VIRSH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn virsh_script(dir: &std::path::Path, code: i32) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("virsh");
        let mut body =
            String::from("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/argv\"\n");
        if code != 0 {
            body.push_str("echo migrate-failed >&2\n");
        }
        body.push_str(&format!("exit {code}\n"));
        std::fs::write(&path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    struct ClearEnv(&'static str);
    impl Drop for ClearEnv {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }

    #[tokio::test]
    async fn migrate_records_virsh_argv_and_surfaces_failure() {
        let _lock = VIRSH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("pistack-virsh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let ok = virsh_script(&root.join("ok"), 0);
        let fail = virsh_script(&root.join("fail"), 1);
        std::env::set_var("PISTACK_VIRSH", &ok);
        let _guard = ClearEnv("PISTACK_VIRSH");

        let bad = migrate("pistack-mach-1", "qemu:///system")
            .await
            .unwrap_err();
        assert!(bad.message.contains("qemu+ssh://"));
        assert!(!root.join("ok").join("argv").exists());
        let spaced = migrate("pistack-mach-1", "qemu+ssh://10.1.0.2/system extra")
            .await
            .unwrap_err();
        assert!(spaced.message.contains("whitespace") || spaced.message.contains("qemu+ssh://"));
        assert!(!root.join("ok").join("argv").exists());

        migrate("pistack-mach-1", "qemu+ssh://10.1.0.2/system")
            .await
            .unwrap();
        let argv = std::fs::read_to_string(root.join("ok").join("argv")).unwrap();
        assert_eq!(
            argv,
            "migrate\n--live\n--p2p\n--tunnelled\npistack-mach-1\nqemu+ssh://10.1.0.2/system\n"
        );

        std::env::set_var("PISTACK_VIRSH", &fail);
        let err = migrate("pistack-mach-1", "qemu+ssh://10.1.0.2/system")
            .await
            .unwrap_err();
        assert!(err.message.contains("migrate-failed"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn overlay_plug_uses_the_po_bridge_and_not_pistack0() {
        let mut overlay = nic("10.88.0.10", true);
        overlay.segment_id = Some(4);
        let xml = hotplug_xml(&overlay).unwrap();
        assert!(xml.contains("<interface type='bridge'>"));
        assert!(xml.contains("<source bridge='po01a102de3f33'/>"));
        assert!(!xml.contains("pistack0"));
        assert!(!xml.contains("network='pistack'"));
        match nic_endpoint(&overlay).unwrap() {
            NicEndpoint::Bridge(name) => assert_eq!(name, "po01a102de3f33"),
            NicEndpoint::Network(_) => panic!("overlay port must not use a libvirt network"),
        }

        let shared = hotplug_xml(&nic("10.88.0.10", true)).unwrap();
        assert!(shared.contains("<source network='pistack'/>"));
        assert!(!shared.contains("pistack0"));
        assert!(!shared.contains("type='bridge'"));
    }

    #[test]
    fn block_volume_must_stay_under_dev_and_disk_args_name_the_source() {
        let dev = Path::new("/dev/vg/lv");
        let canon = Path::new("/dev/dm-0");
        match classify_flags(dev, canon, false, true).unwrap() {
            HostDisk::Block(path) => assert_eq!(path, canon),
            _ => panic!("expected a block device"),
        }
        assert!(classify_flags(Path::new("/tmp/link"), canon, false, true).is_err());
        assert!(classify_flags(dev, Path::new("/etc/passwd"), false, true).is_err());
        assert!(!under_dev(Path::new("/dev")));
        assert!(!under_dev(Path::new("/etc/passwd")));
        assert!(under_dev(dev));

        let file_args = attach_disk_args("dom", "/images/data.raw", "vdb", false, false, true);
        assert_eq!(
            file_args.iter().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "attach-disk",
                "dom",
                "/images/data.raw",
                "vdb",
                "--targetbus",
                "virtio",
                "--subdriver",
                "raw",
                "--config",
                "--live",
            ]
        );
        let block_args = attach_disk_args("dom", "/dev/dm-0", "vdc", true, true, false);
        assert!(block_args
            .windows(2)
            .any(|w| w[0] == "--sourcetype" && w[1] == "block"));
        assert!(block_args
            .windows(2)
            .any(|w| w[0] == "--mode" && w[1] == "readonly"));
        assert!(!block_args.iter().any(|a| a == "--live"));

        let secret = "super-secret-value";
        let disk = RbdDisk {
            pool: "volumes".into(),
            image: "vol-1".into(),
            user: "pistack".into(),
            monitors: vec![("10.1.0.5".into(), 6789)],
            keyring: Some(PathBuf::from("/tmp/keyring")),
        };
        let xml = rbd_disk_xml(&disk, "vdb", false);
        assert!(xml.contains("protocol='rbd'"));
        assert!(xml.contains("name='volumes/vol-1'"));
        assert!(xml.contains("name='10.1.0.5'"));
        assert!(xml.contains("port='6789'"));
        assert!(!xml.contains(secret));
        assert!(!xml.contains("keyring"));
        assert!(parse_monitor("10.1.0.5").unwrap() == ("10.1.0.5".into(), 6789));
        assert!(parse_monitor("not a host").is_err());
        assert!(keyring_file(&Some(secret.into())).is_err());
        let err = keyring_file(&Some(secret.into())).unwrap_err();
        assert!(!err.message.contains(secret));
    }

    #[tokio::test]
    async fn classify_path_accepts_a_file_and_rejects_a_char_device() {
        let file = std::env::temp_dir().join(format!("pistack-class-{}", std::process::id()));
        std::fs::write(&file, b"disk").unwrap();
        match classify_path(&file.to_string_lossy()).await.unwrap() {
            HostDisk::File(path) => assert_eq!(path, file),
            _ => panic!("expected a file"),
        }
        let err = classify_path("/dev/null").await.unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::BadRequest);
        assert!(classify_path("relative.raw").await.is_err());
        assert!(classify_path("/tmp/../etc/passwd").await.is_err());
        assert!(classify_path("/dev/pistack-no-such-lv").await.is_err());
        let _ = std::fs::remove_file(&file);
    }

    fn write_virsh(dir: &std::path::Path) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("virsh");
        let out = dir.to_string_lossy().replace('\'', "'\\''");
        let body = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" >> '{out}/calls'
printf '\n' >> '{out}/calls'
cmd=""
skip=0
for a in "$@"; do
  if [ "$skip" = 1 ]; then
    skip=0
    continue
  fi
  if [ "$a" = "-c" ]; then
    skip=1
    continue
  fi
  cmd="$a"
  break
done
case "$cmd" in
  domstate)
    if [ -f '{out}/missing' ]; then
      echo "error: failed to get domain" >&2
      exit 1
    fi
    echo running
    ;;
  domiflist)
    if [ -f '{out}/iflist' ]; then
      cat '{out}/iflist'
    fi
    ;;
  domblklist)
    if [ -f '{out}/blklist' ]; then
      cat '{out}/blklist'
    fi
    ;;
  attach-device)
    for a in "$@"; do
      if [ -f "$a" ]; then
        cp "$a" '{out}/device.xml'
      fi
    done
    ;;
  attach-disk)
    printf '%s\n' "$@" > '{out}/attach-disk'
    ;;
  detach-interface)
    printf '%s\n' "$@" > '{out}/detach'
    ;;
  *)
    echo "unexpected: $cmd" >&2
    exit 1
    ;;
esac
exit 0
"#
        );
        std::fs::write(&path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn port_req(address: &str, segment: Option<u32>) -> PortPlug {
        let mut nic = nic(address, true);
        nic.segment_id = segment;
        PortPlug {
            machine: req("ubuntu").id,
            nic,
        }
    }

    #[tokio::test]
    async fn plug_and_unplug_use_the_overlay_bridge_and_a_missing_domain_is_unavailable() {
        let _lock = VIRSH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pistack-plug-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let bin = write_virsh(&root);
        std::env::set_var("PISTACK_VIRSH", &bin);
        let _guard = ClearEnv("PISTACK_VIRSH");

        std::fs::write(root.join("missing"), b"1").unwrap();
        let missing = plug_port(&port_req("10.90.0.10", Some(4)))
            .await
            .unwrap_err();
        assert_eq!(missing.code, pistack_types::error::ErrorCode::Unavailable);
        let calls = std::fs::read_to_string(root.join("calls")).unwrap();
        assert!(calls.contains("domstate"));
        assert!(!calls.contains("attach-device"));
        assert!(!calls.contains("migrate"));
        let _ = std::fs::remove_file(root.join("missing"));
        let _ = std::fs::remove_file(root.join("calls"));

        plug_port(&port_req("10.88.0.10", Some(4))).await.unwrap();
        ();
        let xml = std::fs::read_to_string(root.join("device.xml")).unwrap();
        assert!(xml.contains("<source bridge='po01a102de3f33'/>"));
        assert!(!xml.contains("pistack0"));
        assert!(!xml.contains("network='pistack'"));
        let calls = std::fs::read_to_string(root.join("calls")).unwrap();
        assert!(!calls.contains("net-start"));
        assert!(!calls.contains("net-define"));
        let _ = std::fs::remove_file(root.join("calls"));
        let _ = std::fs::remove_file(root.join("device.xml"));

        std::fs::write(
            root.join("iflist"),
            " Interface   Type     Source           Model    MAC\n\
             -------------------------------------------------------\n\
              vnet0       bridge   pistack0         virtio   52:54:00:aa:bb:cc\n",
        )
        .unwrap();
        plug_port(&port_req("10.90.0.10", Some(4))).await.unwrap();
        let xml = std::fs::read_to_string(root.join("device.xml")).unwrap();
        assert!(xml.contains("bridge='po01a102de3f33'"));
        assert!(!xml.contains("pistack0"));
        let detach = std::fs::read_to_string(root.join("detach")).unwrap();
        assert!(detach.contains("detach-interface"));
        assert!(detach.contains("bridge"));
        assert!(detach.contains("52:54:00:aa:bb:cc"));
        assert!(!detach.contains("pistack0"));
        let _ = std::fs::remove_file(root.join("device.xml"));
        let _ = std::fs::remove_file(root.join("detach"));
        let _ = std::fs::remove_file(root.join("calls"));

        std::fs::write(
            root.join("iflist"),
            " Interface   Type     Source             Model    MAC\n\
             -------------------------------------------------------\n\
              vnet0       bridge   po01a102de3f33     virtio   52:54:00:aa:bb:cc\n",
        )
        .unwrap();
        plug_port(&port_req("10.90.0.10", Some(4))).await.unwrap();
        assert!(!root.join("device.xml").exists());

        unplug_port(&port_req("10.90.0.10", Some(4))).await.unwrap();
        let detach = std::fs::read_to_string(root.join("detach")).unwrap();
        assert!(detach.contains("52:54:00:aa:bb:cc"));
        assert!(detach.contains("bridge"));
        assert!(!detach.contains("pistack0"));

        std::fs::write(root.join("iflist"), "").unwrap();
        let _ = std::fs::remove_file(root.join("detach"));
        unplug_port(&port_req("10.90.0.10", Some(4))).await.unwrap();
        assert!(!root.join("detach").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    fn write_rbd(dir: &std::path::Path, code: i32) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("rbd");
        let out = dir.to_string_lossy().replace('\'', "'\\''");
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{out}/rbd-args'\nif [ {code} -ne 0 ]; then echo rbd-failed >&2; exit {code}; fi\nexit 0\n"
        );
        std::fs::write(&path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[tokio::test]
    async fn rbd_attach_uses_the_tool_and_a_missing_tool_is_unavailable() {
        let _lock = VIRSH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!(
            "pistack-rbd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let virsh_bin = write_virsh(&root.join("virshd"));
        let rbd = write_rbd(&root.join("rbdd"), 0);
        let images = root.join("images");
        std::fs::create_dir_all(&images).unwrap();
        let prev_virsh = std::env::var_os("PISTACK_VIRSH");
        let prev_rbd = std::env::var_os("PISTACK_RBD_BIN");
        let prev_dir = std::env::var_os("PISTACK_LIBVIRT_DIR");
        std::env::set_var("PISTACK_VIRSH", &virsh_bin);
        std::env::set_var("PISTACK_RBD_BIN", &rbd);
        std::env::set_var("PISTACK_LIBVIRT_DIR", &images);

        let secret = "super-secret-value";
        let machine = req("ubuntu").id;
        let volume = Id::parse(kind::VOLUME, "vol-018f2c3a-7b6e-7a11-8c22-123456789abc").unwrap();
        let req = VolumeAttach {
            machine: machine.clone(),
            volume: volume.clone(),
            path: String::new(),
            device: None,
            read_only: false,
            target: Some(DiskTarget::Rbd {
                pool: "volumes".into(),
                image: "vol-1".into(),
                monitors: vec!["10.1.0.5:6789".into()],
                user: "pistack".into(),
                key_secret: Some(secret.into()),
            }),
        };
        let rt = LibvirtRuntime;
        let missing_key = rt.attach_volume(&req).await.unwrap_err();
        assert_eq!(
            missing_key.code,
            pistack_types::error::ErrorCode::Unavailable
        );
        assert!(!missing_key.message.contains(secret));
        assert!(!root.join("rbdd").join("rbd-args").exists());

        let mut ok = req.clone();
        if let Some(DiskTarget::Rbd { key_secret, .. }) = &mut ok.target {
            *key_secret = None;
        }
        let dev = rt.attach_volume(&ok).await.unwrap();
        assert_eq!(dev, "vdb");
        let args = std::fs::read_to_string(root.join("rbdd").join("rbd-args")).unwrap();
        assert!(args.contains("info"));
        assert!(args.contains("volumes/vol-1"));
        assert!(!args.contains(secret));
        let xml = std::fs::read_to_string(root.join("virshd").join("device.xml")).unwrap();
        assert!(xml.contains("protocol='rbd'"));
        assert!(xml.contains("volumes/vol-1"));
        assert!(!xml.contains(secret));
        assert!(!xml.contains("keyring"));

        std::env::set_var("PISTACK_RBD_BIN", root.join("no-such-rbd"));
        let gone = rt.attach_volume(&ok).await.unwrap_err();
        assert_eq!(gone.code, pistack_types::error::ErrorCode::Unavailable);

        let file = images.join("data.raw");
        std::fs::write(&file, b"raw").unwrap();
        let _ = std::fs::remove_file(root.join("virshd").join("calls"));
        let file_req = VolumeAttach {
            machine,
            volume,
            path: file.to_string_lossy().into_owned(),
            device: None,
            read_only: false,
            target: None,
        };
        let dev = rt.attach_volume(&file_req).await.unwrap();
        assert_eq!(dev, "vdb");
        let recorded = std::fs::read_to_string(root.join("virshd").join("attach-disk")).unwrap();
        assert!(recorded.contains("attach-disk"));
        assert!(recorded.contains("--subdriver"));
        assert!(!recorded.contains("--sourcetype"));
        assert!(!recorded.contains("migrate"));

        match prev_virsh {
            Some(v) => std::env::set_var("PISTACK_VIRSH", v),
            None => std::env::remove_var("PISTACK_VIRSH"),
        }
        match prev_rbd {
            Some(v) => std::env::set_var("PISTACK_RBD_BIN", v),
            None => std::env::remove_var("PISTACK_RBD_BIN"),
        }
        match prev_dir {
            Some(v) => std::env::set_var("PISTACK_LIBVIRT_DIR", v),
            None => std::env::remove_var("PISTACK_LIBVIRT_DIR"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
