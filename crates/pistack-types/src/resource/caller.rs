// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Caller`: who is making the request, as resolved by `gate`.

use serde::{Deserialize, Serialize};

use super::id::Id;

/// Who is making the request, as resolved by `gate` and attached to every
/// request by the gateway. Modules read this; only `gate` constructs it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Caller {
    pub principal: Id,
    pub principal_name: String,
    /// Project scope of the token, if any.
    pub project: Option<Id>,
    pub roles: Vec<String>,
    /// System-scoped (cloud admin) token.
    pub system: bool,
}

impl Caller {
    pub fn has_role(&self, r: &str) -> bool {
        self.roles.iter().any(|x| x == r)
    }
    /// Cloud admin: holds `admin` at system scope. The `admin` role bound
    /// inside one project does not count; it writes there like a `member`.
    pub fn is_admin(&self) -> bool {
        self.system
    }
    /// Cloud admin, or `admin` or `member` in the token's project: may
    /// create, change and delete. A `reader`, or a caller with no role, may
    /// only read.
    pub fn can_write(&self) -> bool {
        self.system || self.has_role("admin") || self.has_role("member")
    }
    /// [`Caller::can_write`] as an error for handlers and middleware.
    pub fn require_write(&self) -> crate::ApiResult<()> {
        if self.can_write() {
            Ok(())
        } else if self.has_role("reader") {
            Err(crate::ApiError::forbidden("reader role is read-only")
                .with_detail("principal", self.principal_name.as_str()))
        } else {
            Err(crate::ApiError::forbidden("member role required")
                .with_detail("principal", self.principal_name.as_str()))
        }
    }
}
