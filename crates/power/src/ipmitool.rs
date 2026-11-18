// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult};

use crate::bmc_host::bmc_host;
use crate::bmc_user::bmc_user;
use crate::chassis_power::ChassisPower;
use crate::pass_file::PassFile;
use crate::power_control::PowerControl;
use crate::run::run;
use crate::secret::Secret;
use crate::tool::tool;

/// IPMI over `lanplus`, driven by `ipmitool`.
///
/// `ipmitool` reads the password from a file given with `-f`. A
/// [`Secret::Bytes`] is written to a private file for the length of one
/// command and removed, so it never appears on the command line.
pub struct Ipmitool {
    pub(crate) address: String,
    pub(crate) user: String,
    pub(crate) secret: Secret,
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
    pub(crate) async fn call(&self, words: &[&str], what: &str) -> ApiResult<String> {
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
    pub(crate) fn password_file(&self) -> ApiResult<PassFile> {
        match &self.secret {
            Secret::File(path) => Ok(PassFile::Borrowed(path.clone())),
            Secret::Bytes(_) => {
                let password = self.secret.password()?;
                PassFile::from_bytes(password.as_bytes())
            }
        }
    }
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
