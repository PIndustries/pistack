// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Extension, Path, State},
    Json,
};
use pistack_types::{ApiResult, Caller};

use super::caller::caller;
use super::listed_object::ListedObject;
use super::load_bucket::load_bucket;
use super::object_list::ObjectList;

pub(crate) async fn list_objects(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ObjectList>> {
    let bucket = load_bucket(&c, &caller(ext)?, &id)?;
    let objs = c.list_objects(&bucket.meta.id)?;
    let keys = objs.iter().map(|o| o.spec.key.clone()).collect::<Vec<_>>();
    let items = objs
        .into_iter()
        .map(|o| ListedObject {
            id: o.meta.id,
            key: o.spec.key,
            size_bytes: o.status.size_bytes,
            sha256: o.status.sha256,
            version: o.status.version,
        })
        .collect::<Vec<_>>();
    let total = items.len();
    Ok(Json(ObjectList { keys, items, total }))
}
