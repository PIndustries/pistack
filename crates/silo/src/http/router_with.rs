// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::s3;
use crate::s3_create_bucket;
use crate::s3_delete_bucket;
use crate::sigv4::{self, SigV4State};
use axum::{
    extract::DefaultBodyLimit,
    middleware::{self},
    routing::get,
    Router,
};
use pistack_types::ApiError;

use super::app_state::AppState;
use super::create_bucket::create_bucket;
use super::delete_bucket::delete_bucket;
use super::delete_object::delete_object;
use super::get_bucket::get_bucket;
use super::get_object::get_object;
use super::info::info;
use super::list_buckets::list_buckets;
use super::list_objects::list_objects;
use super::peer_blob::peer_blob;
use super::peer_part::peer_part;
use super::peer_part_pull::peer_part_pull;
use super::peer_pull::peer_pull;
use super::peer_upload::peer_upload;
use super::peer_upload_pull::peer_upload_pull;
use super::peer_uploads::peer_uploads;
use super::put_object::put_object;
use super::require_caller::require_caller;

/// Full router: bearer token and (when `keep` is set) AWS SigV4 via keep
/// access keys. `region` is the SigV4 region name ([`SiloConfig::region`]).
pub fn router_with(client: Client, keep: Option<keep::Client>, region: String) -> Router {
    let state = AppState {
        client,
        sigv4: SigV4State { keep, region },
    };
    // Object puts enforce the size cap while streaming; disable axum's
    // default 2 MiB buffer limit the same way mold does for image uploads.
    let object_routes = axum::routing::put(put_object)
        .get(get_object)
        .delete(delete_object)
        .layer(DefaultBodyLimit::disable());
    let s3_object = axum::routing::put(s3::put_object)
        .get(s3::get_object)
        .post(s3::post_object)
        .delete(s3::delete_object)
        .head(s3::head_object)
        .layer(DefaultBodyLimit::disable());

    let peer = Router::new()
        .route("/peer/blobs/{bucket}/{sha256}", get(peer_blob))
        .route(
            "/peer/pull/{object}/{sha256}",
            axum::routing::post(peer_pull),
        )
        .route("/peer/uploads", get(peer_uploads))
        .route(
            "/peer/uploads/{bucket}/{upload}",
            get(peer_upload).post(peer_upload_pull),
        )
        .route(
            "/peer/uploads/{bucket}/{upload}/{part}",
            get(peer_part).post(peer_part_pull),
        )
        .with_state(state.clone());

    Router::new()
        .route("/", get(info))
        .route("/buckets", get(list_buckets).post(create_bucket))
        .route("/buckets/{id}", get(get_bucket).delete(delete_bucket))
        .route("/buckets/{id}/objects", get(list_objects))
        .route("/buckets/{id}/objects/{*key}", object_routes)
        .route(
            "/{bucket}",
            get(s3::list_or_bucket)
                .post(s3::delete_objects)
                .head(s3::head_bucket)
                .put(s3_create_bucket::create_bucket)
                .delete(s3_delete_bucket::delete_bucket),
        )
        .route("/{bucket}/{*key}", s3_object)
        .fallback(|| async { ApiError::not_found("no such silo endpoint") })
        // Inner: demand a Caller. Outer: SigV4 may attach one first.
        .layer(middleware::from_fn(require_caller))
        .layer(middleware::from_fn_with_state(
            state.sigv4.clone(),
            sigv4::middleware,
        ))
        .with_state(state)
        // After the layers: a peer has a join token, not a Caller.
        .merge(peer)
}
