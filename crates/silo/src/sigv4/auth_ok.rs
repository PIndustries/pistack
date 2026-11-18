// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::StreamingAuth;
use pistack_types::Caller;

pub(crate) struct AuthOk {
    pub(crate) caller: Caller,
    pub(crate) expected_payload_sha256: Option<String>,
    pub(crate) streaming: Option<StreamingAuth>,
}
