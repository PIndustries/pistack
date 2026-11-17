// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Every kind relay owns, for the reconciler's event filter.

use super::listener::LISTENER;
use super::member::MEMBER;
use super::pool::POOL;
use pistack_types::kind;

/// Every kind relay owns, for the reconciler's event filter.
pub const KINDS: &[&str] = &[kind::BALANCER, LISTENER, POOL, MEMBER];
