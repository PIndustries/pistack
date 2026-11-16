// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::MAX_IMAGE_BYTES;
use crate::runtime::youki::image::apply_layer;
use crate::runtime::youki::image::blob_file;
use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::finish_dir_modes;
use crate::runtime::youki::image::read_config_file;
use crate::runtime::youki::image::read_json;
use crate::runtime::youki::image::ImageConfig;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::collections::BTreeMap;
use std::path::Path;

pub(in crate::runtime::youki) fn apply_oci(layout: &Path, rootfs: &Path) -> ApiResult<ImageConfig> {
    let index = read_json(&layout.join("index.json"))?;
    let digest = index
        .get("manifests")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|m| m.get("digest"))
        .and_then(|d| d.as_str())
        .ok_or_else(cannot_unpack)?;
    let manifest = read_json(&blob_file(layout, digest)?)?;
    let cfg = match manifest
        .get("config")
        .and_then(|c| c.get("digest"))
        .and_then(|d| d.as_str())
    {
        Some(digest) => read_config_file(&blob_file(layout, digest)?)?,
        None => ImageConfig::default(),
    };
    let layers = manifest
        .get("layers")
        .and_then(|v| v.as_array())
        .ok_or_else(cannot_unpack)?;
    let mut budget = MAX_IMAGE_BYTES;
    let mut dir_modes = BTreeMap::new();
    for layer in layers {
        if layer
            .get("mediaType")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .contains("zstd")
        {
            return Err(ApiError::unavailable(
                "youki runtime: image layer compression is not available",
            ));
        }
        let digest = layer
            .get("digest")
            .and_then(|d| d.as_str())
            .ok_or_else(cannot_unpack)?;
        apply_layer(
            &blob_file(layout, digest)?,
            rootfs,
            &mut budget,
            &mut dir_modes,
        )?;
    }
    finish_dir_modes(rootfs, &dir_modes)?;
    Ok(cfg)
}
