// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The handle: what it holds and how it is opened.

mod ca;
mod ca_bundle_pem;
mod client;
mod limits;
mod open;
mod open_with;
mod root_key;
mod store;
mod with_limits;

pub use client::Client;
