// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The routes.

use super::apply_kit::apply_kit;
use super::apply_stack::apply_stack;
use super::apply_workflow::apply_workflow;
use super::create_kit::create_kit;
use super::create_stack::create_stack;
use super::create_workflow::create_workflow;
use super::delete_kit::delete_kit;
use super::delete_stack::delete_stack;
use super::delete_workflow::delete_workflow;
use super::get_kit::get_kit;
use super::get_stack::get_stack;
use super::get_workflow::get_workflow;
use super::info::info;
use super::list_kits::list_kits;
use super::list_stacks::list_stacks;
use super::list_workflows::list_workflows;
use super::require_caller::require_caller;
use super::rollback_stack::rollback_stack;
use super::update_stack::update_stack;
use crate::client::Client;
use axum::{middleware, routing::get};

pub fn router(client: Client) -> axum::Router {
    axum::Router::new()
        .route("/", get(info))
        .route("/stacks", get(list_stacks).post(create_stack))
        .route(
            "/stacks/{id}",
            get(get_stack).put(update_stack).delete(delete_stack),
        )
        .route("/stacks/{id}/apply", axum::routing::post(apply_stack))
        .route("/stacks/{id}/rollback", axum::routing::post(rollback_stack))
        .route("/kits", get(list_kits).post(create_kit))
        .route("/kits/{id}", get(get_kit).delete(delete_kit))
        .route("/kits/{id}/apply", axum::routing::post(apply_kit))
        .route("/workflows", get(list_workflows).post(create_workflow))
        .route("/workflows/{id}", get(get_workflow).delete(delete_workflow))
        .route("/workflows/{id}/apply", axum::routing::post(apply_workflow))
        .layer(middleware::from_fn(require_caller))
        .with_state(client)
}
