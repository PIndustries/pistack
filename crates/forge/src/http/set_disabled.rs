// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use pistack_types::{ApiResult, Id};

use super::app_state::AppState;
use super::hosts::hosts;

/// Stop placing machines on a host. Calling it again changes the reason
/// when one is given and is otherwise a no-op.
pub(crate) fn set_disabled(s: &AppState, id: &Id, reason: Option<String>) -> ApiResult<Host> {
    Ok(hosts(s).modify(id, |h| {
        h.spec.disabled = true;
        if let Some(r) = &reason {
            h.spec.disabled_reason = r.clone();
        }
        Ok(())
    })?)
}
