// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{extract::FromRequestParts, http::request::Parts};
use pistack_types::{ApiError, Caller, Id};

use crate::http::app_state::AppState;

/// Who is asking and what they may see.
#[derive(Clone, Debug)]
pub struct Scope {
    pub project: Option<Id>,
    pub admin: bool,
    pub caller: Option<Caller>,
}

impl Scope {
    pub(crate) fn filter(&self) -> Option<&Id> {
        self.project.as_ref()
    }
    pub(crate) fn can_see(&self, project: Option<&Id>) -> bool {
        self.admin || (self.project.is_some() && project == self.project.as_ref())
    }
}

impl FromRequestParts<AppState> for Scope {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match parts.extensions.get::<Caller>() {
            Some(c) => Ok(Scope {
                project: c.project.clone(),
                admin: c.is_admin(),
                caller: Some(c.clone()),
            }),
            None if state.dev_admin => Ok(Scope {
                project: None,
                admin: true,
                caller: None,
            }),
            None => Err(ApiError::unauthorized("no credentials")),
        }
    }
}
