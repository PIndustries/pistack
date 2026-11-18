// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

use super::qemu_format_specific::QemuFormatSpecific;

#[derive(Deserialize)]
pub(crate) struct QemuInfo {
    #[serde(rename = "virtual-size")]
    pub(crate) virtual_size: Option<u64>,
    pub(crate) format: Option<String>,
    #[serde(rename = "backing-filename")]
    pub(crate) backing_filename: Option<String>,
    #[serde(rename = "full-backing-filename")]
    pub(crate) full_backing_filename: Option<String>,
    #[serde(rename = "format-specific")]
    pub(crate) format_specific: Option<QemuFormatSpecific>,
}
