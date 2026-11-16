// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::apply_docker;
use crate::runtime::youki::image::apply_oci;
use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::chmod_writable;
use crate::runtime::youki::image::decompress_to_tar;
use crate::runtime::youki::image::docker_manifest;
use crate::runtime::youki::image::extract_tar;
use crate::runtime::youki::image::temp_path;
use crate::runtime::youki::image::ImageConfig;
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::spec::reset_dir;
use pistack_types::ApiResult;
use std::path::Path;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn unpack_archive(
    src: &Path,
    rootfs: &Path,
    privilege: &Privilege,
) -> ApiResult<ImageConfig> {
    let owned = decompress_to_tar(src)?;
    let staging = temp_path("stage");
    let _stage = TempPath(staging.clone());
    std::fs::create_dir_all(&staging).map_err(|_| cannot_unpack())?;
    extract_tar(&owned.path, &staging, false)?;
    if docker_manifest(&staging) {
        apply_docker(&staging, rootfs)
    } else if staging.join("oci-layout").is_file() && staging.join("index.json").is_file() {
        apply_oci(&staging, rootfs)
    } else {
        reset_dir(rootfs, privilege)?;
        extract_tar(&owned.path, rootfs, true)?;
        Ok(ImageConfig::default())
    }
}

pub(in crate::runtime::youki) struct TempPath(PathBuf);

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        chmod_writable(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(in crate::runtime::youki) struct OwnedTar {
    pub(in crate::runtime::youki) path: PathBuf,
    pub(in crate::runtime::youki) temporary: bool,
}

impl Drop for OwnedTar {
    fn drop(&mut self) {
        if self.temporary {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
