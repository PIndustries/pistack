// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::image_config;
use crate::runtime::youki::image::read_json;
use crate::runtime::youki::image::ImageConfig;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn read_config_file(path: &Path) -> ApiResult<ImageConfig> {
    Ok(image_config(&read_json(path)?))
}
