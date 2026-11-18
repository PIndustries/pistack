// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};

use crate::http::api::Api;
use crate::http::delete_mode::DeleteMode;
use crate::http::validate_pod_spec::validate_pod_spec;

impl Api for JobSpec {
    type Status = JobStatus;
    const NOUN: &'static str = "jobs";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, current: Option<&Job>) -> ApiResult<()> {
        validate_pod_spec(&self.template.spec)?;
        if self.template.spec.restart_policy == RestartPolicy::Always {
            return Err(ApiError::bad_request(
                "job pods must use restart_policy OnFailure or Never",
            ));
        }
        if self.completions == 0 || self.parallelism == 0 {
            return Err(ApiError::bad_request(
                "completions and parallelism must be > 0",
            ));
        }
        if let Some(cur) = current {
            if self.template != cur.spec.template {
                return Err(ApiError::unprocessable("job template is immutable"));
            }
        }
        Ok(())
    }
}
