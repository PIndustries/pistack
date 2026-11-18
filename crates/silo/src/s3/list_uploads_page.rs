// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::{self};

pub(crate) struct ListUploadsPage<'a> {
    pub(crate) bucket: &'a str,
    pub(crate) prefix: &'a str,
    pub(crate) key_marker: &'a str,
    pub(crate) upload_id_marker: &'a str,
    pub(crate) delimiter: &'a str,
    pub(crate) max_uploads: usize,
    pub(crate) entries: &'a [multipart::ListUploadEntry],
    pub(crate) truncated: bool,
    pub(crate) encode_url: bool,
}
