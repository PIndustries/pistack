// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use hickory_proto::op::ResponseCode;
use hickory_proto::rr::Record;

/// Outcome of a lookup, before it is turned into a message.
#[derive(Debug)]
pub struct Answer {
    pub code: ResponseCode,
    pub answers: Vec<Record>,
    /// NS records on a positive answer, the SOA on NODATA and NXDOMAIN.
    pub authorities: Vec<Record>,
}
