// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::MAX_IMAGE_BYTES;
use crate::runtime::youki::image::apply_oci;
use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::copy_tree;
use crate::runtime::youki::image::unpack_archive;
use crate::runtime::youki::image::ImageConfig;
use crate::runtime::youki::privilege::Privilege;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn unpack_image_with(
    src: &Path,
    rootfs: &Path,
    privilege: &Privilege,
) -> ApiResult<ImageConfig> {
    let meta = std::fs::symlink_metadata(src).map_err(|_| cannot_unpack())?;
    if meta.file_type().is_symlink() {
        return Err(cannot_unpack());
    }
    if meta.is_dir() {
        if src.join("oci-layout").is_file() {
            return apply_oci(src, rootfs);
        }
        copy_tree(src, rootfs)?;
        return Ok(ImageConfig::default());
    }
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_IMAGE_BYTES {
        return Err(cannot_unpack());
    }
    unpack_archive(src, rootfs, privilege)
}
