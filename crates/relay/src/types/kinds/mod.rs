// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The id kinds relay owns, the set of them the reconciler filters on, and
//! the label it marks its own resources with. Declared `pub mod kinds` so
//! callers write `types::kinds::LISTENER`, as they always did. One file per
//! constant.

mod kinds_const;
mod listener;
mod managed_label;
mod member;
mod pool;

pub use kinds_const::KINDS;
pub use listener::LISTENER;
pub use managed_label::MANAGED_LABEL;
pub use member::MEMBER;
pub use pool::POOL;
