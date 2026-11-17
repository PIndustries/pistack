// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The routes, and the three kinds that get them from [`crud`]. Members are
//! wired by hand because a member is reached through its pool.

use super::create_member::create_member;
use super::delete_member::delete_member;
use super::get_member::get_member;
use super::info::info;
use super::list_members::list_members;
use super::list_pool_members::list_pool_members;
use super::require_caller::require_caller;
use super::update_member::update_member;
use crate::client::Client;
use crate::types::{kinds, Balancer, BalancerSpec, Listener, ListenerSpec, Pool, PoolSpec};
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware;
use axum::response::Response;
use axum::routing::get;
use axum::Json;
use pistack_types::{ApiResult, Caller, Id, Kind, ListQuery, Page};

use super::body::{Create, ScopeQuery, Update};
use super::created::created;
use super::if_match::if_match;
use super::meta::{apply_meta, new_meta};
use super::page::page;
use super::scope::{create_project, scope};
use super::visible::visible;

/// Listeners and pools both belong to a balancer, so both can be listed
/// through one of them by name or id.
fn by_balancer<K: Kind>(
    c: &Client,
    project: Option<&Id>,
    sq: &ScopeQuery,
    items: &mut Vec<K>,
    balancer_of: fn(&K) -> &Id,
) -> ApiResult<()> {
    if let Some(b) = &sq.balancer {
        let lb = c.balancers().resolve(project, b)?;
        items.retain(|k| balancer_of(k) == &lb.meta.id);
    }
    Ok(())
}
use pistack_types::kind;

pub fn router(client: Client) -> axum::Router {
    axum::Router::new()
        .route("/", get(info))
        .route("/balancers", get(list_balancers).post(create_balancer))
        .route(
            "/balancers/{id}",
            get(get_balancer)
                .put(update_balancer)
                .delete(delete_balancer),
        )
        .route("/listeners", get(list_listeners).post(create_listener))
        .route(
            "/listeners/{id}",
            get(get_listener)
                .put(update_listener)
                .delete(delete_listener),
        )
        .route("/pools", get(list_pools).post(create_pool))
        .route(
            "/pools/{id}",
            get(get_pool).put(update_pool).delete(delete_pool),
        )
        .route(
            "/pools/{id}/members",
            get(list_pool_members).post(create_member),
        )
        .route(
            "/pools/{id}/members/{member}",
            get(get_member).put(update_member).delete(delete_member),
        )
        .route("/members", get(list_members))
        .layer(middleware::from_fn(require_caller))
        .with_state(client)
}

crud!(
    Balancer,
    BalancerSpec,
    kind::BALANCER,
    balancers,
    |_, _, _, _| Ok(()),
    list_balancers,
    create_balancer,
    get_balancer,
    update_balancer,
    delete_balancer,
    create_balancer,
    update_balancer,
    delete_balancer,
    await
);
crud!(
    Listener,
    ListenerSpec,
    kinds::LISTENER,
    listeners,
    |c, p, sq, items| by_balancer(c, p, sq, items, |l: &Listener| &l.spec.balancer),
    list_listeners,
    create_listener,
    get_listener,
    update_listener,
    delete_listener,
    create_listener,
    update_listener,
    delete_listener,
    await
);
crud!(
    Pool,
    PoolSpec,
    kinds::POOL,
    pools,
    |c, p, sq, items| by_balancer(c, p, sq, items, |x: &Pool| &x.spec.balancer),
    list_pools,
    create_pool,
    get_pool,
    update_pool,
    delete_pool,
    create_pool,
    update_pool,
    delete_pool
);
