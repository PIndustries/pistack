// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `(ttl, sorted records)` of one set, as hashed into the zone digest.
pub(crate) type SetShape = (Option<u32>, Vec<String>);
