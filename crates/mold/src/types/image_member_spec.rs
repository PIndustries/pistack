// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::{Deserialize, Serialize};

/// Grants `project` access to `image` when its visibility is `Project`.
/// Cloud-scoped record (`meta.project` is the image owner, for listing).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageMemberSpec {
    pub image: Id,
    pub project: Id,
}
