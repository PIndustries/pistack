// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiError, ApiResult, Id};
impl Client {
    pub fn remove_rule(&self, id: &Id, rule: uuid::Uuid) -> ApiResult<SecurityGroup> {
        self.update_security_group(id, None, |sg| {
            let before = sg.spec.rules.len();
            sg.spec.rules.retain(|r| r.id != rule);
            if sg.spec.rules.len() == before {
                return Err(ApiError::not_found(format!("rule {rule} not found")));
            }
            Ok(())
        })
    }
}
