// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::ApiResult;

use crate::http::api::Api;
use crate::http::delete_mode::DeleteMode;
use crate::http::validate_pod_spec::validate_pod_spec;
use crate::http::validate_selector::validate_selector;

impl Api for ReplicaSetSpec {
    type Status = ReplicaSetStatus;
    const NOUN: &'static str = "replicasets";
    const DELETE: DeleteMode = DeleteMode::Annotate;
    fn validate(&mut self, _current: Option<&ReplicaSet>) -> ApiResult<()> {
        validate_pod_spec(&self.template.spec)?;
        validate_selector(&mut self.selector, &self.template)
    }
}
