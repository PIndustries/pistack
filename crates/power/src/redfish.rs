// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult};

use crate::bmc_host::bmc_host;
use crate::bmc_user::bmc_user;
use crate::chassis_power::ChassisPower;
use crate::nanoid::nanoid;
use crate::power_control::PowerControl;
use crate::run::run;
use crate::secret::Secret;
use crate::tool::tool;

/// Redfish, driven by `curl`.
pub struct Redfish {
    pub(crate) address: String,
    pub(crate) user: String,
    pub(crate) secret: Secret,
}

impl Redfish {
    pub fn new(address: impl Into<String>, user: impl Into<String>, secret: Secret) -> Self {
        Self {
            address: address.into(),
            user: user.into(),
            secret,
        }
    }

    pub(crate) fn system(&self) -> String {
        match std::env::var("PISTACK_REDFISH_SYSTEM") {
            Ok(v) if !v.trim().is_empty() => v.trim().trim_matches('/').to_owned(),
            _ => "Systems/1".to_owned(),
        }
    }

    pub(crate) fn base(&self) -> ApiResult<String> {
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
    pub(crate) async fn request(
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

    pub(crate) async fn reset(&self, reset: &str) -> ApiResult<()> {
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
