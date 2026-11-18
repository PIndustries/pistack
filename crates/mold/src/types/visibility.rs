// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

/// Who can see and boot an image.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Owning project only.
    #[default]
    Private,
    /// Owning project plus `ImageMember` projects.
    Project,
    /// Everyone.
    Public,
}
