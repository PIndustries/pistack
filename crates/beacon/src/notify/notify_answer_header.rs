// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use hickory_proto::op::{Metadata, OpCode, ResponseCode};

/// The header of a NOTIFY answer: the request's id, the response bit, the notify
/// opcode kept, AA set, and `code` as the rcode. A NOTIFY answer says nothing
/// beyond that it arrived, so it carries the request's question and no records.
pub(crate) fn notify_answer_header(request: &Metadata, code: ResponseCode) -> Metadata {
    let mut out = Metadata::response_from_request(request);
    out.op_code = OpCode::Notify;
    out.authoritative = true;
    out.response_code = code;
    out
}
