// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

use super::qemu_format_data::QemuFormatData;

#[derive(Deserialize)]
pub(crate) struct QemuFormatSpecific {
    pub(crate) data: Option<QemuFormatData>,
}
