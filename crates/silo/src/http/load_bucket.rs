// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiError, ApiResult, Caller};

use super::visible::visible;

pub(crate) fn load_bucket(client: &Client, caller: &Caller, name_or_id: &str) -> ApiResult<Bucket> {
    if !caller.is_admin() && caller.project.is_none() {
        return Err(ApiError::forbidden("token has no project scope"));
    }
    visible(
        caller,
        client.get_bucket(caller.project.as_ref(), name_or_id)?,
    )
}
