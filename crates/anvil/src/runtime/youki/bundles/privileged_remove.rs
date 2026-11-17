// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `sudo -n <helper> delete --path <path>`. The path is checked here, and
/// the helper checks it again.
use crate::runtime::youki::bundles::checked_removal_path;
use crate::runtime::youki::bundles::cleanup_helper;
use crate::runtime::youki::bundles::path_exists;
use crate::runtime::youki::bundles::remove_err;
use crate::runtime::youki::consts::CLEANUP_ENV;
use crate::runtime::youki::consts::DEFAULT_CLEANUP;
use crate::runtime::youki::consts::SUDOERS_SAMPLE;
use crate::runtime::youki::lifecycle::stderr_tail;
use crate::runtime::youki::privilege::Privilege;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn privileged_remove(
    path: &Path,
    privilege: &Privilege,
    cause: &std::io::Error,
) -> ApiResult<()> {
    let Privilege::Sudo { sudo } = privilege else {
        return Err(remove_err(path, cause));
    };
    let path = match checked_removal_path(path) {
        Ok(path) => path,
        Err(()) => {
            return Err(ApiError::unavailable(format!(
                "youki runtime: removing {}: {cause}. Refusing privileged removal of a path outside the youki state directory or containing '..'",
                path.display()
            )));
        }
    };
    let Some(helper) = cleanup_helper() else {
        return Err(ApiError::unavailable(format!(
            "youki runtime: removing {}: {cause}. Install {DEFAULT_CLEANUP} and allow it in sudoers (`{SUDOERS_SAMPLE}`), or set {CLEANUP_ENV}",
            path.display()
        )));
    };
    let mut cmd = std::process::Command::new(sudo);
    cmd.arg("-n")
        .arg(&helper)
        .arg("delete")
        .arg("--path")
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let output = crate::spawn_retry::spawn_sync_retrying_busy(|| cmd.output()).map_err(|e| {
        ApiError::unavailable(format!(
            "youki runtime: cannot run cleanup helper {}: {e}",
            helper.display()
        ))
    })?;
    if output.status.success() && !path_exists(&path) {
        return Ok(());
    }
    if output.status.success() {
        return Err(ApiError::unavailable(format!(
            "youki runtime: cleanup helper left {} in place",
            path.display()
        )));
    }
    Err(ApiError::unavailable(format!(
        "youki runtime: removing {} failed: {}",
        path.display(),
        stderr_tail(&output.stderr, output.status)
    )))
}
