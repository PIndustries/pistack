// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

use super::driver_reply::DriverReply;

pub(crate) fn unexpected(op: &str, got: &DriverReply) -> ApiError {
    ApiError::internal(format!("host driver: unexpected reply to {op}: {got:?}"))
}
