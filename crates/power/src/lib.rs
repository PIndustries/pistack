// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Out-of-band power control over a baseboard management controller.
//!
//! Two transports, reached through the same [`PowerControl`] trait so a
//! caller can be tested against a fake:
//!
//! * `ipmi` drives `ipmitool` over `lanplus`.
//! * `redfish` drives a Redfish service with `curl`.
//!
//! The password is a [`Secret`]: an [`Ipmitool`] or [`Redfish`] reads it
//! from a file (a path, never the bytes), while a caller that holds the
//! password in memory hands over a [`Secret::Bytes`]. Either way the
//! password is kept out of the environment, the command line and every log
//! line: `ipmitool` is given a file it reads itself, and `curl` a config
//! file written for the one request and removed after it.

use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult};
use std::path::{Path, PathBuf};

pub(crate) mod spawn_retry;

/// Where the BMC password comes from for one control.
#[derive(Clone, Debug)]
pub enum Secret {
    /// A file whose first line is the password. The tool reads it; the
    /// password never enters this process's memory as an argument.
    File(PathBuf),
    /// The password in memory, for a caller that read it from a secret
    /// service. Never shown, never logged.
    Bytes(Vec<u8>),
}

impl Secret {
    /// A file-based secret. A symlink or a non-file is refused.
    pub fn from_file(path: &Path) -> ApiResult<Secret> {
        let meta = std::fs::symlink_metadata(path)
            .map_err(|_| ApiError::unavailable("bmc password file is missing"))?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(ApiError::unavailable("bmc password file is missing"));
        }
        Ok(Secret::File(path.to_path_buf()))
    }

    /// The password bytes, read from wherever this secret points. Never
    /// returned to a caller that would show them; used only to feed a tool.
    fn read(&self) -> ApiResult<Vec<u8>> {
        match self {
            Secret::File(path) => {
                let bytes = std::fs::read(path)
                    .map_err(|_| ApiError::unavailable("bmc password file is missing"))?;
                Ok(bytes)
            }
            Secret::Bytes(bytes) => Ok(bytes.clone()),
        }
    }

    /// The first non-empty line, trimmed, as the tools expect the password.
    fn password(&self) -> ApiResult<String> {
        let bytes = self.read()?;
        let text = String::from_utf8_lossy(&bytes);
        let line = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("");
        if line.is_empty() {
            return Err(ApiError::unavailable("bmc password is empty"));
        }
        Ok(line.to_owned())
    }
}

/// The observed power state of a chassis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChassisPower {
    On,
    Off,
}

/// Power control for one chassis, addressed by its BMC.
#[async_trait]
pub trait PowerControl: Send + Sync {
    /// The transport, `ipmi` or `redfish`, for reports and logs.
    fn kind(&self) -> &'static str;
    /// Power the chassis on.
    async fn power_on(&self) -> ApiResult<()>;
    /// Power the chassis off. Does not confirm the off state; read it back
    /// with [`PowerControl::power_status`].
    async fn power_off(&self) -> ApiResult<()>;
    /// The power state the BMC reports now.
    async fn power_status(&self) -> ApiResult<ChassisPower>;
    /// Point the next boot at the network. Used by provisioning, not by a
    /// fence.
    async fn boot_pxe(&self) -> ApiResult<()>;
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

fn tool(env: &str, name: &str) -> ApiResult<PathBuf> {
    match std::env::var_os(env) {
        Some(v) if !v.is_empty() => {
            let path = PathBuf::from(v);
            if path.is_file() {
                Ok(path)
            } else {
                Err(ApiError::unavailable(format!("{name} is not installed")))
            }
        }
        Some(_) => Err(ApiError::unavailable(format!("{name} is not installed"))),
        None => {
            which(name).ok_or_else(|| ApiError::unavailable(format!("{name} is not installed")))
        }
    }
}

/// One line of tool stderr, flattened and clipped, for an error message.
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

/// A bare `host` or `host:port` BMC address, checked so it is safe to pass
/// to a tool as one argument.
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

/// A BMC user, checked so it is safe to pass to a tool as one argument.
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

async fn run(bin: &Path, args: &[String], what: &str) -> ApiResult<String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = spawn_retry::output_retrying_busy(&mut cmd)
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ApiError::unavailable(format!("{} is not installed", bin.display()))
            } else {
                ApiError::unavailable(format!("{} failed to start: {e}", bin.display()))
            }
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let detail = clip(&String::from_utf8_lossy(&output.stderr));
        return Err(if detail.is_empty() {
            ApiError::unavailable(format!("{what} failed"))
        } else {
            ApiError::unavailable(format!("{what} failed: {detail}"))
        });
    }
    Ok(stdout)
}

/// IPMI over `lanplus`, driven by `ipmitool`.
///
/// `ipmitool` reads the password from a file given with `-f`. A
/// [`Secret::Bytes`] is written to a private file for the length of one
/// command and removed, so it never appears on the command line.
pub struct Ipmitool {
    address: String,
    user: String,
    secret: Secret,
}

impl Ipmitool {
    pub fn new(address: impl Into<String>, user: impl Into<String>, secret: Secret) -> Self {
        Self {
            address: address.into(),
            user: user.into(),
            secret,
        }
    }

    /// `ipmitool` for these words, with the password behind a file.
    async fn call(&self, words: &[&str], what: &str) -> ApiResult<String> {
        let bin = tool("PISTACK_IPMITOOL", "ipmitool")?;
        let (host, port) = bmc_host(&self.address)?;
        let user = bmc_user(&self.user)?;
        let pass = self.password_file()?;
        let mut args = vec![
            "-I".to_owned(),
            "lanplus".into(),
            "-H".into(),
            host,
            "-U".into(),
            user,
            "-f".into(),
            pass.path()?.to_string_lossy().into_owned(),
        ];
        if let Some(port) = port {
            args.push("-p".into());
            args.push(port.to_string());
        }
        for w in words {
            args.push((*w).to_owned());
        }
        let out = run(&bin, &args, &format!("ipmitool {what}")).await;
        drop(pass);
        out
    }

    /// A file holding the password for one command. A [`Secret::File`] is
    /// used as given; [`Secret::Bytes`] is written to a private temporary
    /// file that is removed when the returned guard drops.
    fn password_file(&self) -> ApiResult<PassFile> {
        match &self.secret {
            Secret::File(path) => Ok(PassFile::Borrowed(path.clone())),
            Secret::Bytes(_) => {
                let password = self.secret.password()?;
                PassFile::from_bytes(password.as_bytes())
            }
        }
    }
}

/// A password file for the length of one command.
enum PassFile {
    /// A file that outlives this command; not removed.
    Borrowed(PathBuf),
    /// A temporary file this guard removes on drop.
    Temp(PathBuf),
}

impl PassFile {
    fn from_bytes(bytes: &[u8]) -> ApiResult<PassFile> {
        let dir = std::env::temp_dir().join(format!("pistack-power-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| {
            ApiError::unavailable(format!("power secret dir {}: {e}", dir.display()))
        })?;
        let path = dir.join(format!("pass-{}", nanoid()));
        std::fs::write(&path, bytes)
            .map_err(|e| ApiError::unavailable(format!("power secret file: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(PassFile::Temp(path))
    }

    fn path(&self) -> ApiResult<&Path> {
        match self {
            PassFile::Borrowed(p) | PassFile::Temp(p) => Ok(p.as_path()),
        }
    }
}

impl Drop for PassFile {
    fn drop(&mut self) {
        if let PassFile::Temp(path) = self {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A short unique suffix, so two calls in one process do not share a file.
fn nanoid() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[async_trait]
impl PowerControl for Ipmitool {
    fn kind(&self) -> &'static str {
        "ipmi"
    }

    async fn power_on(&self) -> ApiResult<()> {
        self.call(&["chassis", "power", "on"], "chassis power on")
            .await
            .map(|_| ())
    }

    async fn power_off(&self) -> ApiResult<()> {
        self.call(&["chassis", "power", "off"], "chassis power off")
            .await
            .map(|_| ())
    }

    async fn power_status(&self) -> ApiResult<ChassisPower> {
        let out = self
            .call(&["chassis", "power", "status"], "chassis power status")
            .await?;
        if out.to_ascii_lowercase().contains("is off") {
            Ok(ChassisPower::Off)
        } else if out.to_ascii_lowercase().contains("is on") {
            Ok(ChassisPower::On)
        } else {
            Err(ApiError::unavailable(
                "ipmitool power status was not understood",
            ))
        }
    }

    async fn boot_pxe(&self) -> ApiResult<()> {
        self.call(&["chassis", "bootdev", "pxe"], "chassis bootdev pxe")
            .await
            .map(|_| ())
    }
}

/// Redfish, driven by `curl`.
pub struct Redfish {
    address: String,
    user: String,
    secret: Secret,
}

impl Redfish {
    pub fn new(address: impl Into<String>, user: impl Into<String>, secret: Secret) -> Self {
        Self {
            address: address.into(),
            user: user.into(),
            secret,
        }
    }

    fn system(&self) -> String {
        match std::env::var("PISTACK_REDFISH_SYSTEM") {
            Ok(v) if !v.trim().is_empty() => v.trim().trim_matches('/').to_owned(),
            _ => "Systems/1".to_owned(),
        }
    }

    fn base(&self) -> ApiResult<String> {
        let raw = self.address.trim();
        if raw.is_empty() || raw.len() > 512 {
            return Err(ApiError::bad_request("bmc address is not a redfish url"));
        }
        if raw.chars().any(|c| {
            c.is_control() || matches!(c, ';' | '|' | '&' | '$' | '`' | '\\' | '"' | '\'' | ' ')
        }) {
            return Err(ApiError::bad_request("bmc address is not a redfish url"));
        }
        if raw.starts_with("http://") || raw.starts_with("https://") {
            let trimmed = raw.trim_end_matches('/');
            if let Some(idx) = trimmed.find("/redfish/v1") {
                return Ok(trimmed[..=idx + "/redfish/v1".len() - 1].to_owned());
            }
            return Ok(format!("{trimmed}/redfish/v1"));
        }
        let (host, port) = bmc_host(raw)?;
        Ok(match port {
            Some(p) => format!("https://{host}:{p}/redfish/v1"),
            None => format!("https://{host}/redfish/v1"),
        })
    }

    /// One Redfish request. `curl` gets the credentials through a config
    /// file written for this call and removed after it, so the password is
    /// not on the command line or in the environment. Returns the response
    /// body.
    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        what: &str,
    ) -> ApiResult<String> {
        let bin = tool("PISTACK_CURL", "curl")?;
        let user = bmc_user(&self.user)?;
        let base = self.base()?;
        let url = format!("{base}/{}", path.trim_start_matches('/'));
        let password = self.secret.password()?;

        let cfg_dir = std::env::temp_dir().join(format!("pistack-power-{}", std::process::id()));
        std::fs::create_dir_all(&cfg_dir).map_err(|e| {
            ApiError::unavailable(format!("redfish curl config {}: {e}", cfg_dir.display()))
        })?;
        let cfg = cfg_dir.join(format!("cfg-{}-{}", std::process::id(), nanoid()));
        let escaped_user = user.replace('\\', "\\\\").replace('"', "\\\"");
        let escaped_pass = password.replace('\\', "\\\\").replace('"', "\\\"");
        let cfg_body = format!("user = \"{escaped_user}:{escaped_pass}\"\n");
        std::fs::write(&cfg, cfg_body.as_bytes()).map_err(|e| {
            ApiError::unavailable(format!("redfish curl config {}: {e}", cfg.display()))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600));
        }

        let mut args: Vec<String> = vec![
            "-sS".into(),
            "-f".into(),
            "-k".into(),
            "--config".into(),
            cfg.to_str()
                .ok_or_else(|| ApiError::internal("redfish curl config path"))?
                .to_owned(),
            "-H".into(),
            "Content-Type: application/json".into(),
            "-X".into(),
            method.to_owned(),
        ];
        if let Some(body) = body {
            args.push("-d".into());
            args.push(body.to_owned());
        }
        args.push(url);
        let out = run(&bin, &args, &format!("redfish {what}")).await;
        let _ = std::fs::remove_file(&cfg);
        out
    }

    async fn reset(&self, reset: &str) -> ApiResult<()> {
        let system = self.system();
        let body = format!(r#"{{"ResetType":"{reset}"}}"#);
        self.request(
            "POST",
            &format!("{system}/Actions/ComputerSystem.Reset"),
            Some(&body),
            &format!("power {reset}"),
        )
        .await
        .map(|_| ())
    }
}

#[async_trait]
impl PowerControl for Redfish {
    fn kind(&self) -> &'static str {
        "redfish"
    }

    async fn power_on(&self) -> ApiResult<()> {
        self.reset("On").await
    }

    async fn power_off(&self) -> ApiResult<()> {
        self.reset("ForceOff").await
    }

    async fn power_status(&self) -> ApiResult<ChassisPower> {
        let system = self.system();
        let body = self.request("GET", &system, None, "power status").await?;
        // `"PowerState":"On"` / `"Off"`, whitespace-insensitive.
        let flat: String = body.chars().filter(|c| !c.is_whitespace()).collect();
        let lower = flat.to_ascii_lowercase();
        if lower.contains("\"powerstate\":\"off\"") {
            Ok(ChassisPower::Off)
        } else if lower.contains("\"powerstate\":\"on\"") {
            Ok(ChassisPower::On)
        } else {
            Err(ApiError::unavailable(
                "redfish power status was not understood",
            ))
        }
    }

    async fn boot_pxe(&self) -> ApiResult<()> {
        let system = self.system();
        let body =
            r#"{"Boot":{"BootSourceOverrideEnabled":"Once","BootSourceOverrideTarget":"Pxe"}}"#;
        self.request("PATCH", &system, Some(body), "boot override pxe")
            .await
            .map(|_| ())
    }
}

/// The out-of-band kind named on a host spec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ipmi,
    Redfish,
}

impl Kind {
    pub fn parse(raw: &str) -> ApiResult<Kind> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ipmi" => Ok(Kind::Ipmi),
            "redfish" => Ok(Kind::Redfish),
            other => Err(ApiError::bad_request(format!(
                "power control kind must be ipmi or redfish, not {other:?}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Ipmi => "ipmi",
            Kind::Redfish => "redfish",
        }
    }

    /// Power control for one chassis, from the kind, address, user and
    /// password source.
    pub fn control(
        self,
        address: impl Into<String>,
        user: impl Into<String>,
        secret: Secret,
    ) -> Box<dyn PowerControl> {
        match self {
            Kind::Ipmi => Box::new(Ipmitool::new(address, user, secret)),
            Kind::Redfish => Box::new(Redfish::new(address, user, secret)),
        }
    }

    /// Check that this transport's tool is present, so a caller can fail
    /// before it does any other work. Same message the command itself
    /// gives when the tool is missing.
    pub fn require_tool(self) -> ApiResult<()> {
        match self {
            Kind::Ipmi => tool("PISTACK_IPMITOOL", "ipmitool").map(|_| ()),
            Kind::Redfish => tool("PISTACK_CURL", "curl").map(|_| ()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tools are chosen by process-wide environment variables, so the
    /// tests that set them run one at a time.
    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    #[test]
    fn kinds_parse() {
        assert_eq!(Kind::parse("ipmi").unwrap(), Kind::Ipmi);
        assert_eq!(Kind::parse("Redfish").unwrap(), Kind::Redfish);
        assert!(Kind::parse("wg26").is_err());
        assert_eq!(Kind::Ipmi.as_str(), "ipmi");
        assert_eq!(Kind::Redfish.as_str(), "redfish");
    }

    #[test]
    fn address_and_user_are_checked() {
        assert_eq!(bmc_host("10.9.1.8").unwrap(), ("10.9.1.8".into(), None));
        assert_eq!(
            bmc_host("10.9.1.8:623").unwrap(),
            ("10.9.1.8".into(), Some(623))
        );
        assert!(bmc_host("10.9.1.8; rm -rf /").is_err());
        assert!(bmc_host("").is_err());
        assert_eq!(bmc_user("admin").unwrap(), "admin");
        assert!(bmc_user("-flag").is_err());
    }

    #[tokio::test]
    async fn ipmi_power_status_reads_off() {
        let _guard = env_lock().lock().await;
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("ipmitool");
        std::fs::write(
            &tool,
            "#!/bin/sh\nif [ \"$1\" = -I ]; then echo 'Chassis Power is off'; fi\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var("PISTACK_IPMITOOL", &tool);
        let ctl = Ipmitool::new(
            "10.9.1.8",
            "root",
            Secret::Bytes(b"s3cret-do-not-log\n".to_vec()),
        );
        assert_eq!(ctl.power_status().await.unwrap(), ChassisPower::Off);
        std::env::remove_var("PISTACK_IPMITOOL");
    }

    #[tokio::test]
    async fn redfish_power_status_reads_json() {
        let _guard = env_lock().lock().await;
        let dir = tempfile::tempdir().unwrap();
        let curl = dir.path().join("curl");
        std::fs::write(
            &curl,
            "#!/bin/sh\ncase \"$*\" in *--config*) for a in \"$@\"; do [ \"$p\" = --config ] && cat \"$a\"; p=$a; done;; esac\nprintf '%s' '{\"PowerState\":\"Off\"}'\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var("PISTACK_CURL", &curl);
        let ctl = Redfish::new(
            "https://bmc.example/redfish/v1",
            "root",
            Secret::Bytes(b"s3cret-do-not-log\n".to_vec()),
        );
        assert_eq!(ctl.power_status().await.unwrap(), ChassisPower::Off);
        std::env::remove_var("PISTACK_CURL");
    }

    #[tokio::test]
    async fn secret_bytes_never_appear_on_the_command_line() {
        let _guard = env_lock().lock().await;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("cli.log");
        let tool = dir.path().join("ipmitool");
        std::fs::write(
            &tool,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$POWER_CLI_LOG\"\necho 'Chassis Power is off'\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var("PISTACK_IPMITOOL", &tool);
        std::env::set_var("POWER_CLI_LOG", &log);
        let ctl = Ipmitool::new(
            "10.9.1.8",
            "root",
            Secret::Bytes(b"s3cret-do-not-log\n".to_vec()),
        );
        ctl.power_status().await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(!text.contains("s3cret-do-not-log"), "{text}");
        assert!(
            text.contains("-f"),
            "ipmitool needs a password file: {text}"
        );
        std::env::remove_var("PISTACK_IPMITOOL");
        std::env::remove_var("POWER_CLI_LOG");
    }
}
