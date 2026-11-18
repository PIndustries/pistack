// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};

use crate::http::api::Api;
use crate::http::delete_mode::DeleteMode;
use crate::http::validate_pod_spec::validate_pod_spec;
use crate::http::validate_selector::validate_selector;

impl Api for DeploymentSpec {
    type Status = DeploymentStatus;
    const NOUN: &'static str = "deployments";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, current: Option<&Deployment>) -> ApiResult<()> {
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
        if let DeploymentStrategy::RollingUpdate {
            max_surge,
            max_unavailable,
        } = &self.strategy
        {
            for (name, v) in [
                ("max_surge", max_surge),
                ("max_unavailable", max_unavailable),
            ] {
                if let IntOrPercent::Percent(p) = v {
                    let ok = p
                        .trim()
                        .strip_suffix('%')
                        .and_then(|n| n.trim().parse::<u32>().ok())
                        .is_some_and(|n| n <= 100);
                    if !ok {
                        return Err(ApiError::bad_request(format!(
                            "strategy.{name} must be an integer or '<0-100>%'"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}
