// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::rel_under;
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::spec::reset_dir;
use crate::runtime::youki::spec::Projected;
use crate::runtime::youki::volumes::projected_bytes;
use crate::runtime::youki::volumes::set_mode;
use crate::runtime::youki::volumes::write_mode;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn write_projected(
    dir: &Path,
    files: &Projected,
    file_mode: u32,
    dir_mode: u32,
    privilege: &Privilege,
) -> ApiResult<()> {
    reset_dir(dir, privilege)?;
    set_mode(dir, dir_mode)?;
    let mut keys: Vec<&String> = files.data.keys().collect();
    for key in files.binary_data.keys() {
        if !files.data.contains_key(key) {
            keys.push(key);
        }
    }
    keys.sort();
    for key in keys {
        let Some(rel) = rel_under(key) else {
            continue;
        };
        let Some(bytes) = projected_bytes(files, key) else {
            continue;
        };
        write_mode(&dir.join(rel), &bytes, file_mode)?;
    }
    Ok(())
}
