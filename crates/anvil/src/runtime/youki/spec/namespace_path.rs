// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::youki) fn namespace_path(ns: &serde_json::Value) -> Option<&str> {
    ns.get("path")
        .and_then(|path| path.as_str())
        .filter(|path| !path.is_empty())
}
