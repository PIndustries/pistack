// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Client` is mold's core and the only writer of image `status`. HTTP
//! handlers call it; `forge` and `slab` use [`Client::open`] and
//! [`Client::resolve`] to boot from and populate volumes with images.

mod client_type;
mod create_image;
mod fetch_err;
mod fetch_source;
mod max_import_bytes;
mod max_redirects;
mod not_found_to_none;
mod redirect_action;
#[cfg(test)]
mod tests;
mod update_image;
mod validate_name;
mod validate_source_url;
pub use client_type::Client;
pub use create_image::CreateImage;
pub use update_image::UpdateImage;
pub use validate_source_url::validate_source_url;
