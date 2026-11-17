// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The policy document: named rules plus a map from action name to rule
//! expression, and the evaluator that answers "may this caller do this
//! action to this target?".
//!
//! Document shape (YAML or JSON; JSON is a subset of YAML):
//!
//! ```yaml
//! rules:
//!   admin:  "role:admin or system"
//!   member: "rule:admin or (project_member and role:member)"
//! actions:
//!   "machine:create": "rule:member"
//!   "machine:read":   "rule:reader"
//! ```
//!
//! Action names are `<kind>:<verb>` by convention. `gate` evaluates one
//! document for the whole cloud; operators override pieces of the default
//! with [`Policy::merge`].

mod policy_document;
mod policy_error;
mod policy_eval;
mod target;
#[cfg(test)]
mod tests;

pub use policy_document::PolicyDocument;
pub use policy_error::PolicyError;
pub use policy_eval::Policy;
pub use target::Target;
