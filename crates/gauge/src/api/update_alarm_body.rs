// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::AlarmSpecUpdate;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize, Default)]
pub(crate) struct UpdateAlarm {
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub(crate) spec: Option<AlarmSpecUpdate>,
}
