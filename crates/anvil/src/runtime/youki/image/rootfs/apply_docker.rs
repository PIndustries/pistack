// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::MAX_IMAGE_BYTES;
use crate::runtime::youki::image::apply_layer;
use crate::runtime::youki::image::cannot_unpack;
use crate::runtime::youki::image::finish_dir_modes;
use crate::runtime::youki::image::read_config_file;
use crate::runtime::youki::image::safe_rel;
use crate::runtime::youki::image::ImageConfig;
use pistack_types::ApiResult;
use std::collections::BTreeMap;
use std::path::Path;

pub(in crate::runtime::youki) fn apply_docker(
    staging: &Path,
    rootfs: &Path,
) -> ApiResult<ImageConfig> {
    let bytes = std::fs::read(staging.join("manifest.json")).map_err(|_| cannot_unpack())?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| cannot_unpack())?;
    let entry = value
        .as_array()
        .and_then(|a| a.first())
        .ok_or_else(cannot_unpack)?;
    let cfg = match entry.get("Config").and_then(|v| v.as_str()) {
        Some(path) => read_config_file(&staging.join(safe_rel(path)?))?,
        None => ImageConfig::default(),
    };
    let layers = entry
        .get("Layers")
        .or_else(|| entry.get("layers"))
        .and_then(|v| v.as_array())
        .ok_or_else(cannot_unpack)?;
    let mut budget = MAX_IMAGE_BYTES;
    let mut dir_modes = BTreeMap::new();
    for layer in layers {
        let rel = layer.as_str().ok_or_else(cannot_unpack)?;
        apply_layer(
            &staging.join(safe_rel(rel)?),
            rootfs,
            &mut budget,
            &mut dir_modes,
        )?;
    }
    finish_dir_modes(rootfs, &dir_modes)?;
    Ok(cfg)
}
