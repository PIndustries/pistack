// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `ParameterDecl`: a parameter a document declares.
use pistack_types::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};

use super::template::is_param_name;
use super::template::PARAM_PREFIX;

/// A parameter a document declares. `spec.parameters` holds the values given
/// for these.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParameterDecl {
    /// Name, without the `param.` a reference carries.
    pub name: String,
    /// Value used when no value was given. Without one, a value is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// What the parameter is for, for whoever reads the stack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl ParameterDecl {
    /// Trim the name to its short spelling and refuse a name no reference can
    /// carry.
    pub(crate) fn normalized(&mut self) -> ApiResult<()> {
        self.name = self.name.trim().trim_start_matches(PARAM_PREFIX).to_owned();
        if !is_param_name(&self.name) {
            return Err(ApiError::bad_request(format!(
                "parameter name '{}' must be 1-64 characters of letters, digits, '.' or '-', starting with a letter",
                self.name
            )));
        }
        Ok(())
    }

    /// The value a reference to this parameter takes when none was given.
    pub(crate) fn resolved_default(&self) -> Option<String> {
        self.default.clone()
    }
}
