// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

pub(crate) type DecodedBodyStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<bytes::Bytes, ApiError>> + Send>>;
