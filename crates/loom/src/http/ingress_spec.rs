// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};

use crate::http::api::Api;
use crate::http::delete_mode::DeleteMode;

impl Api for IngressSpec {
    type Status = IngressStatus;
    const NOUN: &'static str = "ingresses";
    const DELETE: DeleteMode = DeleteMode::Direct;
    fn validate(&mut self, _current: Option<&Ingress>) -> ApiResult<()> {
        if self.rules.is_empty() && self.default_backend.is_none() {
            return Err(ApiError::bad_request(
                "spec.rules or spec.default_backend is required",
            ));
        }
        for r in &self.rules {
            for p in &r.paths {
                if !p.path.starts_with('/') {
                    return Err(ApiError::bad_request(format!(
                        "path '{}' must start with '/'",
                        p.path
                    )));
                }
                if p.backend.service.trim().is_empty() {
                    return Err(ApiError::bad_request("backend.service is required"));
                }
            }
        }
        Ok(())
    }
}
