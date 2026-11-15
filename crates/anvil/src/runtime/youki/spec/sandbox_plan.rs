// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::PodPlan;
use crate::runtime::youki::spec::SandboxDoc;
use crate::runtime::MachineRequest;
use pistack_types::ApiError;
use pistack_types::ApiResult;

pub(in crate::runtime::youki) fn sandbox_plan(req: &MachineRequest) -> ApiResult<Option<PodPlan>> {
    let Some(raw) = req.user_data.as_deref() else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if value.get("kind").and_then(|k| k.as_str()) != Some("PodSandbox") {
        return Ok(None);
    }
    let doc: SandboxDoc = serde_json::from_value(value)
        .map_err(|_| ApiError::unavailable("youki runtime: sandbox document is invalid"))?;
    if doc.kind != "PodSandbox" {
        return Ok(None);
    }
    Ok(Some(PodPlan {
        containers: doc.containers,
        volumes: doc.volumes,
        config_maps: doc.config_maps,
        secrets: doc.secrets,
        volume_paths: doc.volume_paths,
        volume_devices: doc.volume_devices,
    }))
}
