// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The process-wide table of edge publishes, keyed by listener id.

use super::edge_publish::EdgePublish;
use std::collections::HashMap;
use std::sync::OnceLock;

pub(crate) fn edge_published() -> &'static parking_lot::Mutex<HashMap<String, EdgePublish>> {
    static EDGE: OnceLock<parking_lot::Mutex<HashMap<String, EdgePublish>>> = OnceLock::new();
    EDGE.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}
