// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

use super::default_arch::default_arch;
use super::image_format::ImageFormat;
use super::visibility::Visibility;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageSpec {
    pub format: ImageFormat,
    /// Free-form OS hint: "ubuntu-24.04", "rhel-9", "alpine".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default = "default_arch")]
    pub arch: String,
    #[serde(default)]
    pub min_disk_gib: u64,
    #[serde(default)]
    pub min_ram_mib: u64,
    #[serde(default)]
    pub visibility: Visibility,
    /// Where `POST /images/{id}/import` fetches from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}
