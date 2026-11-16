// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;

pub(crate) fn write_pod_mount(bundle: &Path, volume: &str) {
    let plan = serde_json::json!({
        "volumes": [{"name": "data", "source": {"volume": volume}}],
        "containers": [{
            "index": 0,
            "mounts": [{
                "name": "data",
                "mount_path": "/data",
                "read_only": false,
                "sub_path": ""
            }]
        }]
    });
    std::fs::write(
        bundle.join("mount-plan.json"),
        serde_json::to_vec_pretty(&plan).unwrap(),
    )
    .unwrap();
    let dir = bundle.join("containers").join("0");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.json"),
        br#"{"mounts":[{"destination":"/proc","type":"proc","source":"proc"}]}"#,
    )
    .unwrap();
}
