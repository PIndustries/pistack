// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};

use crate::http::api::Api;
use crate::http::delete_mode::DeleteMode;

impl Api for AutoscalerSpec {
    type Status = AutoscalerStatus;
    const NOUN: &'static str = "autoscalers";
    const DELETE: DeleteMode = DeleteMode::Direct;
    fn validate(&mut self, _current: Option<&Autoscaler>) -> ApiResult<()> {
        self.target_name = self.target_name.trim().to_owned();
        self.metric = self.metric.trim().to_owned();
        if self.target_name.is_empty() || self.target_name.len() > 128 {
            return Err(ApiError::bad_request(
                "spec.target_name must be 1-128 characters",
            ));
        }
        if self.metric.is_empty() || self.metric.len() > 256 {
            return Err(ApiError::bad_request(
                "spec.metric must be 1-256 characters",
            ));
        }
        if !self.target_value.is_finite() || self.target_value <= 0.0 {
            return Err(ApiError::bad_request(
                "spec.target_value must be a finite number greater than zero",
            ));
        }
        if self.max_replicas < self.min_replicas {
            return Err(ApiError::bad_request(
                "spec.max_replicas must be >= spec.min_replicas",
            ));
        }
        if self.max_replicas == 0 {
            return Err(ApiError::bad_request(
                "spec.max_replicas must be at least 1",
            ));
        }
        Ok(())
    }
}
