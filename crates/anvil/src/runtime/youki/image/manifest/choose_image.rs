// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::absolute_image;
use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::digest_image;
use crate::runtime::youki::spec::Launch;
use crate::runtime::MachineRequest;
use pistack_types::ApiResult;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn choose_image(
    req: &MachineRequest,
    launch: Option<&Launch>,
) -> ApiResult<PathBuf> {
    if let Some(launch) = launch {
        if let Some(path) = absolute_image(&launch.image) {
            return Ok(path);
        }
    }
    if let Some(path) = absolute_image(&req.image).or_else(|| digest_image(&req.image)) {
        return Ok(path);
    }
    if let Some(launch) = launch {
        if let Some(path) = digest_image(&launch.image) {
            return Ok(path);
        }
    }
    Err(cannot_unpack())
}
