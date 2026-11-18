// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use pistack_types::{kind, ApiError, ApiResult, Caller, Id};

use super::visible::visible;

/// What `?vpc=` asks for: no filter, no vpc, or one named vpc.
pub(crate) enum InWhich {
    Any,
    None,
    One(Id),
}

impl InWhich {
    /// Read `?vpc=`: a vpc's name, its id, or `none`. Admins list across
    /// projects, where a name resolves against no project of its own, so a
    /// name is looked for in the scope first and across the cloud after.
    pub(crate) fn parse(
        c: &Client,
        caller: &Caller,
        project: Option<&Id>,
        want: &Option<String>,
    ) -> ApiResult<InWhich> {
        let Some(w) = want else {
            return Ok(InWhich::Any);
        };
        if w == "none" {
            return Ok(InWhich::None);
        }
        if let Some(id) = Id::parse(kind::VPC, w) {
            return Ok(InWhich::One(visible(caller, c.vpcs().get(&id)?)?.meta.id));
        }
        let vpc = c
            .vpcs()
            .get_by_name(project, w)
            .or_else(|_| c.vpcs().get_by_name(None, w))
            .map_err(|_| ApiError::not_found(format!("no vpc named '{w}'")))?;
        Ok(InWhich::One(visible(caller, vpc)?.meta.id))
    }

    pub(crate) fn holds(&self, of: Option<&Id>) -> bool {
        match self {
            InWhich::Any => true,
            InWhich::None => of.is_none(),
            InWhich::One(id) => of == Some(id),
        }
    }
}
