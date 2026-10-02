// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Policy rule language and evaluator: who may do what.
//!
//! A policy is a YAML/JSON document that maps action names such as
//! `machine:create` to small boolean rule expressions over the caller and
//! the target resource:
//!
//! ```yaml
//! rules:
//!   admin:  "role:admin or system"
//!   member: "rule:admin or (project_member and role:member)"
//! actions:
//!   "machine:create": "rule:member"
//!   "volume:delete":  "rule:member or owner"
//! ```
//!
//! `gate` loads [`Policy::default_policy`], merges the operator's overrides
//! with [`Policy::merge`], and every handler calls [`Policy::authorize`]
//! with the request's [`pistack_types::Caller`] and a [`Target`].
//!
//! See [`expr`] for the exact grammar.

pub mod expr;
pub mod policy;

pub use expr::{Expr, ParseError};
pub use policy::{Policy, PolicyDocument, PolicyError, Target};
