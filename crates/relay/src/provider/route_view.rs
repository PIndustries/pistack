// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! One listener route with its pool already resolved.

use crate::types::{Member, Pool, Route};

/// One listener route with its pool already resolved.
#[derive(Clone, Debug)]
pub struct RouteView {
    pub route: Route,
    pub pool: Pool,
    pub members: Vec<Member>,
}
