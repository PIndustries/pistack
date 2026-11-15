// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::namespace_path;

pub(in crate::runtime::youki) fn creates_user_namespace(namespaces: &[serde_json::Value]) -> bool {
    namespaces.iter().any(|ns| {
        ns.get("type").and_then(|kind| kind.as_str()) == Some("user")
            && namespace_path(ns).is_none()
    })
}
