// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::bundles::privatize_bundle;
use crate::runtime::youki::image::unpack_image_with;
use crate::runtime::youki::network::requested_host_netns;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::spec::process_spec;
use crate::runtime::youki::spec::reset_dir;
use crate::runtime::youki::spec::rootfs_empty;
use crate::runtime::youki::spec::sandbox_namespaces;
use crate::runtime::youki::spec::Launch;
use crate::runtime::youki::spec::Limits;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn install_bundle(
    bundle: &Path,
    image: &Path,
    hostname: &str,
    limits: &Limits,
    launch: Option<&Launch>,
    mode: &RunMode,
    host_netns: Option<&Path>,
) -> ApiResult<()> {
    std::fs::create_dir_all(bundle).map_err(|e| {
        ApiError::unavailable(format!("youki runtime: creating {}: {e}", bundle.display()))
    })?;
    privatize_bundle(bundle, &mode.privilege)?;
    let rootfs = bundle.join("rootfs");
    reset_dir(&rootfs, &mode.privilege)?;
    let cfg = unpack_image_with(image, &rootfs, &mode.privilege)?;
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
    let from_env = requested_host_netns();
    let netns = from_env.as_deref().or(host_netns);
    let text = serde_json::to_vec_pretty(&mode.oci_config(
        Some(hostname),
        limits,
        (&args, &env, &cwd),
        &sandbox_namespaces(mode, netns),
        &[],
    ))
    .map_err(|e| ApiError::internal(format!("youki runtime: encoding config: {e}")))?;
    std::fs::write(bundle.join("config.json"), text)
        .map_err(|e| ApiError::unavailable(format!("youki runtime: writing config.json: {e}")))?;
    privatize_bundle(bundle, &mode.privilege)
}
