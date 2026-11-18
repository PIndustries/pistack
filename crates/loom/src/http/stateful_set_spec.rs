// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};
use std::collections::BTreeSet;

use crate::http::api::Api;
use crate::http::delete_mode::DeleteMode;
use crate::http::validate_name::validate_name;
use crate::http::validate_pod_spec::validate_pod_spec;
use crate::http::validate_selector::validate_selector;

impl Api for StatefulSetSpec {
    type Status = StatefulSetStatus;
    const NOUN: &'static str = "statefulsets";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, current: Option<&StatefulSet>) -> ApiResult<()> {
        validate_pod_spec(&self.template.spec)?;
        if self.template.labels.contains_key(POD_TEMPLATE_HASH) {
            return Err(ApiError::bad_request(format!(
                "spec.template.labels may not set '{POD_TEMPLATE_HASH}'"
            )));
        }
        validate_selector(&mut self.selector, &self.template)?;
        let mut claims = BTreeSet::new();
        for c in &self.volume_claim_templates {
            validate_name(&c.name)
                .map_err(|e| ApiError::bad_request(format!("volume claim name: {}", e.message)))?;
            if c.size_gib == 0 {
                return Err(ApiError::bad_request(format!(
                    "volume claim '{}' size_gib must be at least 1",
                    c.name
                )));
            }
            if !claims.insert(c.name.as_str()) {
                return Err(ApiError::bad_request(format!(
                    "volume claim name '{}' is used twice",
                    c.name
                )));
            }
        }
        if let Some(cur) = current {
            if self.selector != cur.spec.selector {
                return Err(ApiError::unprocessable("spec.selector is immutable"));
            }
            if self.service_name != cur.spec.service_name {
                return Err(ApiError::unprocessable("spec.service_name is immutable"));
            }
            if self.volume_claim_templates != cur.spec.volume_claim_templates {
                return Err(ApiError::unprocessable(
                    "spec.volume_claim_templates is immutable",
                ));
            }
        }
        Ok(())
    }
}
