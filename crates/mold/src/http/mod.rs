// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/image`.
//!
//! `POST /images` makes the record, `PUT /images/{id}/data` streams the
//! bytes in, `GET /images/{id}/data` streams them out. Everything is
//! filtered by the caller's visibility.

mod add_member;
mod caller;
mod create_image;
mod delete_image;
mod download;
mod editable;
mod get_image;
mod if_match;
mod import_body;
mod import_image;
mod internal;
mod list_images;
mod list_members;
mod member_body;
mod page;
mod remove_member;
mod require_caller;
mod router;
mod update_image;
mod upload;
pub use router::router;
