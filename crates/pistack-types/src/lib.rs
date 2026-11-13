// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Shared types for every PiStack module.
//!
//! Four things live here and nowhere else:
//! * the resource model (ids, metadata, the `Resource` envelope),
//! * `ApiError`, the one error shape every HTTP handler returns,
//! * the `Module` contract that `pistack-core` uses to mount each module,
//! * node identity: the one stable name agents, hosts and controllers share.
//!
//! Modules depend on this crate; this crate depends on no module.

pub mod error;
pub mod module;
pub mod node;
pub mod resource;

pub use error::{ApiError, ApiResult};
pub use module::{CancellationToken, Leadership, Module, ModuleContext, ModuleInfo};
pub use resource::Spec;
pub use resource::*;

/// The build number a user sees: `pistack --version`, `pi version`, the
/// gateway root document, what an agent reports, the HTTP user agents.
///
/// A release build is numbered by its date, `<year>.<month>.<day>.<build>`
/// with a two-digit year and the build counting from 1 each day, as in
/// `26.11.12.1`. The release workflow passes it in as `PISTACK_BUILD` when it
/// compiles. Any other build is `dev`. The version in `Cargo.toml` is only
/// there because cargo wants one and is shown nowhere.
pub const BUILD: &str = match option_env!("PISTACK_BUILD") {
    Some(build) if !build.is_empty() => build,
    _ => "dev",
};
