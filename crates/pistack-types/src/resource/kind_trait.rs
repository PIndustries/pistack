// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Kind`: the marker trait the store keys resources by.

use serde::{de::DeserializeOwned, Serialize};

use super::meta::Meta;

/// Marker trait the store keys resources by. Modules normally do not
/// implement this directly: implement [`Spec`](super::spec::Spec) on the spec
/// type and the blanket impl in [`Resource`](super::resource) makes
/// `Resource<S, T>` a `Kind`. (A direct `impl Kind for Resource<MySpec,
/// MyStatus>` from a module crate is rejected by the orphan rule, since both
/// the trait and `Resource` live here.)
pub trait Kind: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    /// Table name in the store and id prefix, e.g. "mach".
    const KIND: &'static str;
    fn meta(&self) -> &Meta;
    fn meta_mut(&mut self) -> &mut Meta;
}
