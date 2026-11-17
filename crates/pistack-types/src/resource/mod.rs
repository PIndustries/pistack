// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The resource model shared by every module.
//!
//! Every object a user can create (a machine, a network, a volume, a pod)
//! is a `Resource<Spec, Status>`: common `Meta` plus a module-owned `spec`
//! (what the user asked for) and `status` (what is true right now). This is
//! deliberately the Kubernetes shape, so `loom` can speak the k8s API later
//! without a translation layer, and so every module gets list/get/watch for
//! free from `pistack-store`.

mod caller;
mod condition;
mod event;
mod event_action;
mod id;
pub mod kind;
mod kind_trait;
mod list_query;
mod meta;
mod page;
mod phase;
#[cfg(test)]
mod tests;

mod resource_envelope;
mod resources;
mod spec;

pub use caller::Caller;
pub use condition::Condition;
pub use event::Event;
pub use event_action::EventAction;
pub use id::Id;
pub use kind_trait::Kind;
pub use list_query::ListQuery;
pub use meta::Meta;
pub use page::Page;
pub use phase::Phase;
pub use resource_envelope::Resource;
pub use resources::Resources;
pub use spec::Spec;
