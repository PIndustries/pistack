// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{kind, ApiError, ApiResult, Id};

use super::already_gone::already_gone;
use super::block_on_async::block_on_async;
use super::request_machine_delete::request_machine_delete;

/// Delete one managed id through its owning module. `Ok(true)` when the
/// row was deleted or marked for async deletion; `Ok(false)` when the kind
/// is unknown to apply.
pub(crate) fn delete_owned(
    store: &Store,
    fabric: &fabric::Client,
    relay: &relay::Client,
    beacon: &beacon::Client,
    id_str: &str,
) -> ApiResult<bool> {
    if let Some(id) = Id::parse(kind::RECORD, id_str) {
        match beacon.delete_recordset(&id) {
            Ok(_) => return Ok(true),
            Err(e) => return already_gone(e).map(|()| true),
        }
    }
    if let Some(id) = Id::parse(relay::kinds::LISTENER, id_str) {
        match block_on_async(relay.delete_listener(&id)) {
            Ok(_) => return Ok(true),
            Err(e) => return already_gone(e).map(|()| true),
        }
    }
    if let Some(id) = Id::parse(kind::POD, id_str) {
        loom::controllers::request_delete(&store.collection::<loom::Pod>(), &id)
            .map_err(ApiError::from)?;
        return Ok(true);
    }
    if let Some(id) = Id::parse(kind::MACHINE, id_str) {
        request_machine_delete(store, &id)?;
        return Ok(true);
    }
    if let Some(id) = Id::parse(kind::ROUTER, id_str) {
        match fabric.delete_router(&id) {
            Ok(_) => return Ok(true),
            Err(e) => return already_gone(e).map(|()| true),
        }
    }
    if let Some(id) = Id::parse(kind::SUBNET, id_str) {
        match fabric.delete_subnet(&id) {
            Ok(_) => return Ok(true),
            Err(e) => return already_gone(e).map(|()| true),
        }
    }
    if let Some(id) = Id::parse(kind::NETWORK, id_str) {
        match fabric.delete_network(&id) {
            Ok(_) => return Ok(true),
            Err(e) => return already_gone(e).map(|()| true),
        }
    }
    Ok(false)
}
