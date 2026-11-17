// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The id kinds relay owns, the set of them the reconciler filters on, and
//! the label it marks its own resources with. Declared `pub mod kinds` so
//! callers write `types::kinds::LISTENER`, as they always did.

use pistack_types::kind;

/// Id kind for a listener, beyond the shared `lb`.
pub const LISTENER: &str = "lsn";
/// Id kind for a pool.
pub const POOL: &str = "pool";
/// Id kind for a member.
pub const MEMBER: &str = "mbr";

/// Every kind relay owns, for the reconciler's event filter.
pub const KINDS: &[&str] = &[kind::BALANCER, LISTENER, POOL, MEMBER];

/// Label relay puts on balancers it creates for other modules.
pub const MANAGED_LABEL: &str = "pistack.io/managed-by";
