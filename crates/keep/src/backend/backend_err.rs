// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::error::KeepError;

pub(in crate::backend) fn backend_err(msg: impl Into<String>) -> KeepError {
    KeepError::Backend(msg.into())
}
