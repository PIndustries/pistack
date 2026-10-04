// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Shared filesystems (NFS and CephFS).
//!
//! A [`Share`] is a name, a protocol (`nfs` or `cephfs`), a size and an
//! access list. NFS with a client CIDR is `Ready` only after `exportfs`
//! accepts it. CephFS is `Ready` only after `ceph fs subvolume create`
//! succeeds. A missing tool stays `Pending`.
//!
//! HTTP is mounted at `/shelf`: `GET|POST /shares`, `GET|PUT|DELETE /shares/{id}`.

mod http;
mod realize;
mod types;

pub use types::*;

use async_trait::async_trait;
use axum::Router;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};

/// The shelf module. Routes read the store from [`ModuleContext`].
#[derive(Default)]
pub struct Shelf;

impl Shelf {
    fn store(ctx: &ModuleContext) -> Store {
        let store = ctx.store::<Store>();
        (*store).clone()
    }
}

#[async_trait]
impl Module for Shelf {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "shelf",
            prefix: "shelf",
            summary: "Shared filesystems",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn router(&self, ctx: ModuleContext) -> Router {
        http::router(Self::store(&ctx))
    }
}
