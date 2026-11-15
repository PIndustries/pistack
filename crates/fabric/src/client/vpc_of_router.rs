// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::Client;
use crate::types::*;
impl Client {
    /// The vpc a router belongs to, when it belongs to one.
    pub(in crate::client) fn vpc_of_router(&self, r: &Router) -> String {
        match &r.spec.vpc {
            Some(v) => crate::vpc::owner_name(r).unwrap_or_else(|| v.to_string()),
            None => String::new(),
        }
    }
}
