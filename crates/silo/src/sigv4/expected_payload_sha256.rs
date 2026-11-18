// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Declared `x-amz-content-sha256` for the request, when it is a hex digest
/// that handlers must check against the streamed body.
#[derive(Clone, Debug)]
pub struct ExpectedPayloadSha256(pub String);
