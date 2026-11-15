// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn sandbox_doc(image: &str) -> String {
    serde_json::json!({
        "kind": "PodSandbox",
        "containers": [
            {
                "name": "web",
                "image": image,
                "command": ["/usr/bin/web"],
                "args": ["--serve"],
                "working_dir": "/work",
                "env": [
                    {"name": "APP_MODE", "value": "prod"},
                    {"name": "SECRET", "value_from": {"secret": "do-not-log-user-data"}}
                ]
            },
            {
                "name": "side",
                "image": image,
                "command": ["/usr/bin/side"],
                "args": ["--watch"]
            }
        ]
    })
    .to_string()
}
