// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::{Deserialize, Serialize};

use super::static_route::StaticRoute;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RouterSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_network: Option<Id>,
    /// Set when the VPC with this id owns the router. Nothing may be changed
    /// on it directly and it cannot be deleted by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vpc: Option<Id>,
    /// Subnets the router has an interface on. Managed with
    /// `PUT /routers/{id}/interfaces`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<StaticRoute>,
}
