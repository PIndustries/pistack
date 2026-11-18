// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};

use crate::http::api::Api;
use crate::http::delete_mode::DeleteMode;
use crate::http::validate_pod_spec::validate_pod_spec;
use crate::http::validate_selector::validate_selector;

impl Api for DaemonSetSpec {
    type Status = DaemonSetStatus;
    const NOUN: &'static str = "daemonsets";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, current: Option<&DaemonSet>) -> ApiResult<()> {
        validate_pod_spec(&self.template.spec)?;
        if self.template.labels.contains_key(POD_TEMPLATE_HASH) {
            return Err(ApiError::bad_request(format!(
                "spec.template.labels may not set '{POD_TEMPLATE_HASH}'"
            )));
        }
        validate_selector(&mut self.selector, &self.template)?;
        if let Some(cur) = current {
            if self.selector != cur.spec.selector {
                return Err(ApiError::unprocessable("spec.selector is immutable"));
            }
        }
        Ok(())
    }
}
