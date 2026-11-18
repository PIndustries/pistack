// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiResult, Resource, Spec};
use serde::{de::DeserializeOwned, Serialize};

use crate::http::delete_mode::DeleteMode;

/// What the generic handlers need from each spec type.
pub trait Api: Spec + Serialize + DeserializeOwned {
    type Status: crate::controllers::Status;
    /// URL noun, plural: `pods`.
    const NOUN: &'static str;
    const DELETE: DeleteMode;
    /// Normalise and validate a spec about to be stored. `current` is the
    /// stored resource on update.
    fn validate(&mut self, current: Option<&Resource<Self, Self::Status>>) -> ApiResult<()>;
}
