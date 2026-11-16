// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(super) mod auth_trailer_checksum;
pub(super) mod requires_chunk_signatures;
// The struct keeps the folder's own name, so it lives in streaming_auth.rs
#[allow(clippy::module_inception)]
pub(super) mod streaming_auth;
pub(super) mod streaming_kind;
