// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
use crate::runtime::youki::image::unpack_image_with;
use crate::runtime::youki::image::ImageConfig;
use crate::runtime::youki::privilege::Privilege;
use pistack_types::ApiResult;
use std::path::Path;

#[cfg(test)]
pub(in crate::runtime::youki) fn unpack_image(src: &Path, rootfs: &Path) -> ApiResult<ImageConfig> {
    unpack_image_with(src, rootfs, &Privilege::Root)
}
