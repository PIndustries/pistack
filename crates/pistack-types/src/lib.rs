// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Shared types for every PiStack module.
//!
//! Three things live here and nowhere else:
//! * the resource model (ids, metadata, the `Resource` envelope),
//! * `ApiError`, the one error shape every HTTP handler returns,
//! * the `Module` contract that `pistack-core` uses to mount each module.
//!
//! Modules depend on this crate; this crate depends on no module.

pub mod error;
pub mod module;
pub mod resource;

pub use error::{ApiError, ApiResult};
pub use module::{CancellationToken, Leadership, Module, ModuleContext, ModuleInfo};
pub use resource::Spec;
pub use resource::*;
