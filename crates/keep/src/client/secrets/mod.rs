// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Secret resources and their sealed payloads.

pub mod create_secret;
pub mod delete_secret;
pub mod get_secret;
pub mod get_secret_payload;
pub mod list_secrets;
pub(in crate::client) mod reserved_labels;
pub mod rotate_secret;
pub mod update_secret;
