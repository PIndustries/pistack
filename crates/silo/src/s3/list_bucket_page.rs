// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) struct ListBucketPage<'a> {
    pub(crate) bucket: &'a str,
    pub(crate) prefix: &'a str,
    pub(crate) delimiter: &'a str,
    pub(crate) start_after: &'a str,
    pub(crate) max_keys: usize,
    pub(crate) entries: &'a [crate::client::ListObjectEntry],
    pub(crate) truncated: bool,
    pub(crate) next_token: Option<&'a str>,
    pub(crate) continuation: Option<&'a str>,
    pub(crate) encode_url: bool,
}
