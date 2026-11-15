// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::data_dir::data_dir;
use super::volume_name::volume_name;
use pistack_types::{ApiResult, Id};
use std::path::PathBuf;

pub(in crate::runtime::volfs) fn volume_paths(volume: &Id) -> ApiResult<(PathBuf, PathBuf)> {
    let name = volume_name(volume)?;
    let root = data_dir()?.join("slab").join("mounts");
    Ok((root.join(&name), root.join(format!("{name}.loop"))))
}
