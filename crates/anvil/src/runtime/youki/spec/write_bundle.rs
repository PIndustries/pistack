// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::choose_image;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::spec::install_bundle;
use crate::runtime::youki::spec::launch_from;
use crate::runtime::youki::spec::Limits;
use crate::runtime::MachineRequest;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) async fn write_bundle(
    bundle: &Path,
    req: &MachineRequest,
    mode: &RunMode,
    host_netns: Option<PathBuf>,
) -> ApiResult<()> {
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
    let mode = mode.clone();
    tokio::task::spawn_blocking(move || {
        install_bundle(
            &bundle,
            &image,
            &hostname,
            &limits,
            launch.as_ref(),
            &mode,
            host_netns.as_deref(),
        )
    })
    .await
    .map_err(|_| ApiError::unavailable("youki runtime: image unpack stopped"))?
}
