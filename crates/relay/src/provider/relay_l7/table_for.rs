// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Building the route table a listener programs: its default pool and one
//! target per route, each with the members it may dial.

use super::live_backends::live_backends;
use super::normalize_host::normalize_host;
use super::table::{RouteTarget, Table};
use crate::provider::ListenerView;
use crate::types::PathMatch;

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
