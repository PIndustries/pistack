// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Default largest object body accepted by a single PUT (5 GiB).
///
/// Matches the historical S3 single-PUT ceiling. Override per client with
/// [`crate::Client::with_max_object_bytes`] or `[modules.silo]
/// max_object_bytes` (see [`SiloConfig`]).
pub const MAX_OBJECT_BYTES: usize = 5 * 1024 * 1024 * 1024;
