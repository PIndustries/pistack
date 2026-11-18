// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::DefaultBodyLimit,
    middleware::{self},
    routing::{get, post},
    Router,
};

use crate::client::Client;

use super::add_member::add_member;
use super::create_image::create_image;
use super::delete_image::delete_image;
use super::download::download;
use super::get_image::get_image;
use super::import_image::import_image;
use super::list_images::list_images;
use super::list_members::list_members;
use super::remove_member::remove_member;
use super::require_caller::require_caller;
use super::update_image::update_image;
use super::upload::upload;

pub fn router(client: Client) -> Router {
    Router::new()
        .route("/images", get(list_images).post(create_image))
        .route(
            "/images/{id}",
            get(get_image).put(update_image).delete(delete_image),
        )
        .route(
            "/images/{id}/data",
            get(download).put(upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/images/{id}/import", post(import_image))
        .route("/images/{id}/members", get(list_members).post(add_member))
        .route(
            "/images/{id}/members/{project}",
            axum::routing::delete(remove_member),
        )
        .layer(middleware::from_fn(require_caller))
        .with_state(client)
}
