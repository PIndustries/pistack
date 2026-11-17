// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Resource<Spec, Status>`: the envelope every module's objects use.

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use super::kind_trait::Kind;
use super::meta::Meta;
use super::spec::Spec;

/// The envelope. `spec` is what was asked for; `status` is observed state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Resource<S, T> {
    #[serde(flatten)]
    pub meta: Meta,
    pub spec: S,
    pub status: T,
}

impl<S, T> Kind for Resource<S, T>
where
    S: Spec,
    T: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
{
    const KIND: &'static str = S::KIND;
    fn meta(&self) -> &Meta {
        &self.meta
    }
    fn meta_mut(&mut self) -> &mut Meta {
        &mut self.meta
    }
}

impl<S: Spec, T: Default> Resource<S, T> {
    /// Convenience constructor: fresh `Meta` of the spec's kind, default status.
    pub fn new(name: impl Into<String>, project: Option<super::id::Id>, spec: S) -> Self {
        Self {
            meta: Meta::new(S::KIND, name, project),
            spec,
            status: T::default(),
        }
    }
}
