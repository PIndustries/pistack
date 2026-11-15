// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::json_string;
use crate::runtime::youki::image::json_strings;
use crate::runtime::youki::image::ImageConfig;

pub(in crate::runtime::youki) fn image_config(value: &serde_json::Value) -> ImageConfig {
    let cfg = value.get("config").unwrap_or(value);
    ImageConfig {
        entrypoint: json_strings(cfg, "Entrypoint", "entrypoint"),
        cmd: json_strings(cfg, "Cmd", "cmd"),
        env: json_strings(cfg, "Env", "env"),
        working_dir: json_string(cfg, "WorkingDir", "workingDir"),
    }
}
