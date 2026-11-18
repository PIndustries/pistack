// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use crate::vpc;
use pistack_types::{ApiResult, Caller};

use super::guarded::Guarded;

impl Guarded for VpcSpec {
    /// The blocks are the vpc: a create that names none has nothing to cut
    /// subnets from.
    fn guard(&self, caller: &Caller, cur: Option<&Self>) -> ApiResult<()> {
        let _ = caller;
        if cur.is_none() {
            vpc::cidrs_required(self)?;
        }
        Ok(())
    }
}
