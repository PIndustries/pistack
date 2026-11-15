// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::file_uid::file_uid;
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::privilege::RunMode;
use std::path::Path;

pub(in crate::runtime::youki) fn network_namespace(
    mode: &RunMode,
    host_netns: Option<&Path>,
) -> serde_json::Value {
    let Some(path) = host_netns else {
        return serde_json::json!({ "type": "network" });
    };
    let path_str = path.to_string_lossy().into_owned();
    match &mode.privilege {
        Privilege::Rootless { uid, .. } => match file_uid(path) {
            Some(owner) if owner == *uid => {
                serde_json::json!({ "type": "network", "path": path_str })
            }
            Some(owner) => {
                tracing::warn!(
                    path = %path.display(),
                    owner,
                    agent = uid,
                    "youki runtime: skipping network namespace owned by another uid; using the sandbox network namespace"
                );
                serde_json::json!({ "type": "network" })
            }
            None => {
                tracing::warn!(
                    path = %path.display(),
                    "youki runtime: skipping network namespace that cannot be inspected; using the sandbox network namespace"
                );
                serde_json::json!({ "type": "network" })
            }
        },
        Privilege::Root | Privilege::Sudo { .. } => {
            serde_json::json!({ "type": "network", "path": path_str })
        }
    }
}
