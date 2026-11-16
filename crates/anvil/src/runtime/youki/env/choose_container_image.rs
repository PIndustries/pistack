// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::absolute_image;
use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::digest_image;
use pistack_types::ApiResult;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn choose_container_image(
    index: usize,
    image: &str,
    fallback: &str,
) -> ApiResult<PathBuf> {
    if let Some(path) = absolute_image(image).or_else(|| digest_image(image)) {
        return Ok(path);
    }
    if index != 0 {
        return Err(cannot_unpack());
    }
    absolute_image(fallback)
        .or_else(|| digest_image(fallback))
        .ok_or_else(cannot_unpack)
}
