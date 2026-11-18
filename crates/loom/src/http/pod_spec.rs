// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};

use crate::http::api::Api;
use crate::http::delete_mode::DeleteMode;
use crate::http::validate_pod_spec::validate_pod_spec;

impl Api for PodSpec {
    type Status = PodStatus;
    const NOUN: &'static str = "pods";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, current: Option<&Pod>) -> ApiResult<()> {
        validate_pod_spec(self)?;
        if let Some(cur) = current {
            if cur.status.machine.is_some() && *self != cur.spec {
                return Err(ApiError::unprocessable(
                    "pod spec is immutable once its machine exists; only labels and annotations may change",
                ));
            }
        }
        Ok(())
    }
}
