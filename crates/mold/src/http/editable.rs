// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult, Caller};

use crate::client::Client;
use crate::types::*;

pub(crate) fn editable(c: &Client, caller: &Caller, id: &str) -> ApiResult<Image> {
    let img = c.get_visible(caller, id)?;
    if !c.can_edit(caller, &img) {
        return Err(ApiError::forbidden(format!(
            "image {} belongs to another project",
            img.meta.id
        )));
    }
    Ok(img)
}
