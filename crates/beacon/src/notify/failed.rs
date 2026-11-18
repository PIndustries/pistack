// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use hickory_proto::op::{Header, HeaderCounts, Metadata};
use hickory_server::server::ResponseInfo;

/// A `ResponseInfo` for a header that never reached the peer.
pub(crate) fn failed(metadata: Metadata) -> ResponseInfo {
    ResponseInfo::from(Header {
        metadata,
        counts: HeaderCounts::default(),
    })
}
