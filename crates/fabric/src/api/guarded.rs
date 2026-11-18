// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiResult, Caller};

/// Checked before a create (`cur` is `None`) or an update.
pub(crate) trait Guarded: Sized {
    fn guard(&self, _caller: &Caller, _cur: Option<&Self>) -> ApiResult<()> {
        Ok(())
    }
}
