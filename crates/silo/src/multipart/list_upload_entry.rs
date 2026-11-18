// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::upload_summary::UploadSummary;

/// One row in a ListMultipartUploads page (upload or rolled-up prefix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListUploadEntry {
    Upload(UploadSummary),
    CommonPrefix(String),
}

impl ListUploadEntry {
    pub fn sort_key(&self) -> &str {
        match self {
            Self::Upload(u) => u.key.as_str(),
            Self::CommonPrefix(p) => p.as_str(),
        }
    }
}
