// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::ListObjectEntry;

pub(crate) struct VersionsPage<'a> {
    pub(crate) bucket: &'a str,
    pub(crate) prefix: &'a str,
    pub(crate) delimiter: &'a str,
    pub(crate) key_marker: &'a str,
    pub(crate) version_id_marker: &'a str,
    pub(crate) max_keys: usize,
    pub(crate) entries: &'a [ListObjectEntry],
    pub(crate) truncated: bool,
    pub(crate) next_key: Option<&'a str>,
    pub(crate) next_version: Option<String>,
    pub(crate) encode_url: bool,
    pub(crate) all: Vec<crate::types::Object>,
}
