// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiError, ApiResult, Id};
impl Client {
    pub fn add_rule(&self, id: &Id, rule: Rule) -> ApiResult<SecurityGroup> {
        self.update_security_group(id, None, |sg| {
            if sg.spec.rules.iter().any(|r| r.id == rule.id) {
                return Err(ApiError::conflict(format!(
                    "rule {} already exists",
                    rule.id
                )));
            }
            sg.spec.rules.push(rule);
            Ok(())
        })
    }
}
