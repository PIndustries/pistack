// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::DEFAULT_PATH;
use crate::runtime::youki::consts::PAUSE_ELF;
use crate::runtime::youki::network::requested_host_netns;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::spec::reset_dir;
use crate::runtime::youki::spec::sandbox_namespaces;
use crate::runtime::youki::spec::Limits;
use crate::runtime::youki::volumes::write_json;
use crate::runtime::youki::volumes::write_mode;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn write_pause_rootfs(
    bundle: &Path,
    hostname: &str,
    limits: &Limits,
    mode: &RunMode,
    host_netns: Option<&Path>,
) -> ApiResult<()> {
    let rootfs = bundle.join("rootfs");
    reset_dir(&rootfs, &mode.privilege)?;
    write_mode(&rootfs.join("pause"), PAUSE_ELF, 0o755)?;
    let args = vec!["/pause".to_string()];
    let env = vec![format!("PATH={DEFAULT_PATH}")];
    let from_env = requested_host_netns();
    let netns = from_env.as_deref().or(host_netns);
    let spec = mode.oci_config(
        Some(hostname),
        limits,
        (&args, &env, "/"),
        &sandbox_namespaces(mode, netns),
        &[],
    );
    write_json(&bundle.join("config.json"), &spec, 0o644)
}
