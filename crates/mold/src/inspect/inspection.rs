// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

use super::detected_format::DetectedFormat;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    pub format: DetectedFormat,
    /// Disk size the image presents to the guest. `None` when unknown.
    pub virtual_size: Option<u64>,
    pub backing_file: Option<String>,
    pub data_file: Option<String>,
    /// `qemu-img` or `header`.
    pub source: &'static str,
}

impl Inspection {
    /// The security check. Backing files and external data files are
    /// never allowed in uploaded images.
    pub fn reject_unsafe(&self) -> ApiResult<()> {
        if let Some(b) = &self.backing_file {
            return Err(ApiError::unprocessable(format!(
                "image has a backing file ('{b}'); upload a flattened image (qemu-img convert -O qcow2)"
            )));
        }
        if let Some(d) = &self.data_file {
            return Err(ApiError::unprocessable(format!(
                "image has an external data file ('{d}'); not allowed"
            )));
        }
        Ok(())
    }
}
