// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::bind_mount;
use crate::runtime::youki::volumes::read_io;
use crate::runtime::youki::volumes::write_json;
use crate::runtime::youki::volumes::OciBind;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use std::path::Path;

pub(in crate::runtime::youki) fn patch_mounts(config: &Path, binds: &[OciBind]) -> ApiResult<()> {
    let bytes = match std::fs::read(config) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(read_io(config, e)),
    };
    let mut spec: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        ApiError::unavailable(format!("youki runtime: reading {}: {e}", config.display()))
    })?;
    let mut mounts = Vec::new();
    if let Some(existing) = spec.get("mounts").and_then(|m| m.as_array()) {
        for mount in existing {
            let dest = mount
                .get("destination")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            if dest == "/proc" || dest == "/dev" || dest == "/sys" {
                mounts.push(mount.clone());
            }
        }
    }
    for bind in binds {
        mounts.push(bind_mount(bind));
    }
    let obj = spec.as_object_mut().ok_or_else(|| {
        ApiError::unavailable(format!(
            "youki runtime: reading {}: config is not an object",
            config.display()
        ))
    })?;
    obj.insert("mounts".into(), serde_json::Value::Array(mounts));
    write_json(config, &spec, 0o600)
}
