// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::io::Read;

use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::gunzip;
use crate::runtime::youki::image::temp_path;
use crate::runtime::youki::image::OwnedTar;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn decompress_to_tar(src: &Path) -> ApiResult<OwnedTar> {
    let mut file = std::fs::File::open(src).map_err(|_| cannot_unpack())?;
    let mut magic = [0u8; 4];
    let n = file.read(&mut magic).unwrap_or(0);
    if n >= 4 && magic == [0x28, 0xB5, 0x2F, 0xFD] {
        return Err(ApiError::unavailable(
            "youki runtime: image layer compression is not available",
        ));
    }
    if n >= 2 && magic[0] == 0x1f && magic[1] == 0x8b {
        let dst = temp_path("tar");
        gunzip(src, &dst)?;
        return Ok(OwnedTar {
            path: dst,
            temporary: true,
        });
    }
    Ok(OwnedTar {
        path: src.to_path_buf(),
        temporary: false,
    })
}
