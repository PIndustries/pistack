// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::BTreeMap;
use std::sync::Arc;

use super::overlay_sent::OverlaySent;

pub(crate) type OverlaySentMap = Arc<parking_lot::Mutex<BTreeMap<String, OverlaySent>>>;
