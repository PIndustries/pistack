// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};

use crate::http::api::Api;
use crate::http::delete_mode::DeleteMode;

impl Api for ConfigMapSpec {
    type Status = ConfigMapStatus;
    const NOUN: &'static str = "configmaps";
    const DELETE: DeleteMode = DeleteMode::Direct;
    fn validate(&mut self, current: Option<&ConfigMap>) -> ApiResult<()> {
        if let Some(cur) = current {
            if cur.spec.immutable
                && (self.data != cur.spec.data || self.binary_data != cur.spec.binary_data)
            {
                return Err(ApiError::unprocessable("configmap is immutable"));
            }
        }
        Ok(())
    }
}
