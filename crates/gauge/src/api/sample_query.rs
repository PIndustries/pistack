// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

#[derive(Deserialize, Default)]
pub(crate) struct SampleQuery {
    #[serde(default)]
    pub(crate) metric: Option<String>,
}
