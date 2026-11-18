// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use std::collections::BTreeSet;

use crate::http::validate_name::validate_name;
use crate::http::validate_probe::validate_probe;
use crate::types::PodSpec;

pub(crate) fn validate_pod_spec(spec: &PodSpec) -> ApiResult<()> {
    if spec.containers.is_empty() {
        return Err(ApiError::bad_request("spec.containers must not be empty"));
    }
    let mut names = BTreeSet::new();
    for c in &spec.containers {
        validate_name(&c.name)
            .map_err(|e| ApiError::bad_request(format!("container name: {}", e.message)))?;
        if !names.insert(c.name.as_str()) {
            return Err(ApiError::bad_request(format!(
                "container name '{}' is used twice",
                c.name
            )));
        }
        if c.image.trim().is_empty() {
            return Err(ApiError::bad_request(format!(
                "container '{}' needs an image",
                c.name
            )));
        }
        if let Some(probe) = &c.liveness_probe {
            validate_probe(&c.name, "liveness_probe", probe)?;
        }
        if let Some(probe) = &c.readiness_probe {
            validate_probe(&c.name, "readiness_probe", probe)?;
        }
        for vm in &c.volume_mounts {
            if !spec.volumes.iter().any(|v| v.name == vm.name) {
                return Err(ApiError::bad_request(format!(
                    "container '{}' mounts unknown volume '{}'",
                    c.name, vm.name
                )));
            }
        }
    }
    let mut vols = BTreeSet::new();
    for v in &spec.volumes {
        if !vols.insert(v.name.as_str()) {
            return Err(ApiError::bad_request(format!(
                "volume name '{}' is used twice",
                v.name
            )));
        }
    }
    Ok(())
}
