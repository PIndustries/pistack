// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::error::{KeepError, Result};

pub(in crate::client) fn denied(r: Result<impl std::fmt::Debug>) -> pistack_types::ApiError {
    match r {
        Err(KeepError::Denied(e)) => e,
        other => panic!("expected a quota refusal, got {other:?}"),
    }
}
