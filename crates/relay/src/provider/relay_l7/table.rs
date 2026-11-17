// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Which member a request goes to: the route table built from a listener
//! view, host and path ranking, and the backup and health rules that decide
//! what a pool offers.

use super::http::normalize_host;
use crate::picker::{Backend, Picker};

use crate::provider::ListenerView;
use crate::types::{Member, PathMatch};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
/// The members to choose between. A backup member is only used when no
/// primary is left; the rest of the health and weight rules are the
/// picker's.
fn live_backends(members: &[Member]) -> Vec<Backend> {
    let primary: Vec<Backend> = members
        .iter()
        .filter(|m| !m.spec.backup)
        .map(Backend::of)
        .collect();
    if !primary.is_empty() {
        return primary;
    }
    members.iter().map(Backend::of).collect()
}

pub(super) fn table_for(view: &ListenerView) -> Table {
    if !view.listener.spec.admin_up {
        return Table::default();
    }
    let default = match &view.pool {
        Some((_, members)) => live_backends(members),
        None => Vec::new(),
    };
    let routes = view
        .routes
        .iter()
        .map(|r| RouteTarget {
            host: r.route.host.as_deref().map(normalize_host),
            path: r.route.path.clone(),
            exact: r.route.path_match == PathMatch::Exact,
            members: if r.pool.spec.admin_up {
                live_backends(&r.members)
            } else {
                Vec::new()
            },
        })
        .collect();
    Table { default, routes }
}

/// Where requests on one listener go: routes first, then the default pool.
#[derive(Default)]
pub(super) struct Table {
    pub(super) default: Vec<Backend>,
    pub(super) routes: Vec<RouteTarget>,
}

pub(super) struct RouteTarget {
    pub(super) host: Option<String>,
    pub(super) path: String,
    pub(super) exact: bool,
    pub(super) members: Vec<Backend>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Choice {
    Member(SocketAddr),
    /// A route or the default pool applies but has no member up.
    Unavailable,
    /// Routes exist, none matches, and there is no default member.
    NotFound,
}

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

/// 2 for an exact host, 1 for a wildcard, 0 for a route with no host.
fn host_rank(route: Option<&str>, request: Option<&str>) -> Option<u8> {
    let Some(want) = route else {
        return Some(0);
    };
    let have = request?;
    if let Some(suffix) = want.strip_prefix('*') {
        let label = have.strip_suffix(suffix)?;
        return (!label.is_empty() && !label.contains('.')).then_some(1);
    }
    (want == have).then_some(2)
}

/// Length of the matched route path, or `None` when it does not match.
fn path_len(route: &str, exact: bool, request: &str) -> Option<usize> {
    if exact {
        return (route == request).then_some(route.len());
    }
    let base = route.trim_end_matches('/');
    if base.is_empty() {
        return Some(0);
    }
    let matched = request == base
        || request
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'));
    matched.then_some(base.len())
}

/// One backend with the connections this proxy holds on it.
pub(super) fn weighed(
    backends: &[Backend],
    conns: &parking_lot::Mutex<HashMap<SocketAddr, u64>>,
) -> Vec<Backend> {
    let open = conns.lock();
    backends
        .iter()
        .map(|b| Backend {
            conns: open.get(&b.addr).copied().unwrap_or(0),
            ..*b
        })
        .collect()
}

pub(super) fn opened(conns: &parking_lot::Mutex<HashMap<SocketAddr, u64>>, addr: SocketAddr) {
    *conns.lock().entry(addr).or_insert(0) += 1;
}

pub(super) fn closed(conns: &parking_lot::Mutex<HashMap<SocketAddr, u64>>, addr: SocketAddr) {
    let mut open = conns.lock();
    if let Some(n) = open.get_mut(&addr) {
        *n = n.saturating_sub(1);
        if *n == 0 {
            open.remove(&addr);
        }
    }
}

/// One connection counted against a backend until the request holding it
/// is over, whichever way the request ends.
pub(super) struct Held(
    pub(super) Arc<parking_lot::Mutex<HashMap<SocketAddr, u64>>>,
    pub(super) SocketAddr,
);

impl Drop for Held {
    fn drop(&mut self) {
        closed(&self.0, self.1);
    }
}
