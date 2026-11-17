// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Matching a request against the table: the best route by host then path
//! length, else the default pool, else nothing.

use super::choice::Choice;
use super::host_rank::host_rank;
use super::path_len::path_len;
use super::table::{RouteTarget, Table};
use super::weighed::weighed;
use crate::picker::{Backend, Picker};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

impl Table {
    pub(super) fn choose(
        &self,
        host: Option<&str>,
        path: &str,
        client: Option<IpAddr>,
        picker: &Picker,
        conns: &parking_lot::Mutex<HashMap<SocketAddr, u64>>,
    ) -> Choice {
        let take = |members: &[Backend]| match picker.pick(&weighed(members, conns), client) {
            Some(addr) => Choice::Member(addr),
            None => Choice::Unavailable,
        };
        let mut best: Option<((u8, usize, bool), &RouteTarget)> = None;
        for route in &self.routes {
            let Some(host_rank) = host_rank(route.host.as_deref(), host) else {
                continue;
            };
            let Some(len) = path_len(&route.path, route.exact, path) else {
                continue;
            };
            let rank = (host_rank, len, route.exact);
            if best.as_ref().is_none_or(|(have, _)| rank > *have) {
                best = Some((rank, route));
            }
        }
        match best {
            Some((_, route)) => take(&route.members),
            None if self.routes.is_empty() || !self.default.is_empty() => take(&self.default),
            None => Choice::NotFound,
        }
    }
}
