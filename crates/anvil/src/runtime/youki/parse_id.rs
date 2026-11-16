// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::kind;
use pistack_types::ApiError;
use pistack_types::ApiResult;
use pistack_types::Id;

pub(super) fn parse_id(s: &str) -> ApiResult<Id> {
    Id::parse(kind::MACHINE, s).ok_or_else(|| {
        ApiError::unavailable(format!("youki runtime: state id {s} is not a machine id"))
    })
}
