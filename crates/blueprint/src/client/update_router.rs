// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiResult, Id};
use serde_json::Value;
use std::collections::BTreeSet;

use super::about::about;
use super::router_spec::router_spec;

/// Routes and the external network go through fabric's router update;
/// the interface set through `set_router_interfaces`.
pub(crate) fn update_router(
    fabric: &fabric::Client,
    store: &Store,
    project: Option<&Id>,
    cur: &fabric::Router,
    name: &str,
    raw: &Value,
) -> ApiResult<bool> {
    let spec = router_spec(store, project, name, raw)?;
    let mut changed = false;
    if spec.external_network != cur.spec.external_network || spec.routes != cur.spec.routes {
        let (external_network, routes) = (spec.external_network.clone(), spec.routes.clone());
        fabric
            .update_router(&cur.meta.id, Some(cur.meta.revision), |r| {
                r.spec.external_network = external_network;
                r.spec.routes = routes;
                Ok(())
            })
            .map_err(about("router", name))?;
        changed = true;
    }
    let want: BTreeSet<&Id> = spec.interfaces.iter().collect();
    let have: BTreeSet<&Id> = cur.spec.interfaces.iter().collect();
    if want != have {
        fabric
            .set_router_interfaces(&cur.meta.id, None, &spec.interfaces)
            .map_err(about("router", name))?;
        changed = true;
    }
    Ok(changed)
}
