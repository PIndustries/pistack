// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};

/// Default an empty selector to the template labels and require it to match.
pub(crate) fn validate_selector(selector: &mut Selector, template: &PodTemplate) -> ApiResult<()> {
    if selector.is_empty() {
        if template.labels.is_empty() {
            return Err(ApiError::bad_request(
                "spec.selector is empty and spec.template.labels has nothing to default it from",
            ));
        }
        *selector = Selector::from_labels(&template.labels);
    }
    if !selector.matches(&template.labels) {
        return Err(ApiError::bad_request(
            "spec.selector does not match spec.template.labels",
        ));
    }
    Ok(())
}
