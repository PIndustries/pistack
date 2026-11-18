// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{extract::FromRequestParts, http::request::Parts};
use pistack_types::{ApiError, ListQuery};

use super::serde_urlencoded_lite::serde_urlencoded_lite;

/// `Query<ListQuery>` whose rejection is an `ApiError`.
pub struct List(pub ListQuery);

impl<S: Send + Sync> FromRequestParts<S> for List {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let q = parts.uri.query().unwrap_or("");
        let v: ListQuery = serde_urlencoded_lite(q)?;
        Ok(List(v))
    }
}
