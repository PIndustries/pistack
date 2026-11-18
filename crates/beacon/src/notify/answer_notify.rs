// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use hickory_proto::op::ResponseCode;
use hickory_server::server::{Request, ResponseHandler, ResponseInfo};
use hickory_server::zone_handler::MessageResponseBuilder;

use super::failed::failed;
use super::notify_answer_header::notify_answer_header;

/// Answer a NOTIFY that reached the handler with [`notify_answer_header`]. The
/// question is handed over as it arrived and every record section is left
/// empty, so the bytes written to the socket are exactly that header.
pub(crate) async fn answer_notify<R: ResponseHandler>(
    request: &Request,
    code: ResponseCode,
    response_handle: &mut R,
) -> ResponseInfo {
    let mut metadata = notify_answer_header(&request.metadata, code);
    let response = MessageResponseBuilder::new(&request.queries, request.edns.as_ref())
        .build_no_records(metadata);
    match response_handle.send_response(response).await {
        Ok(info) => info,
        Err(e) => {
            tracing::warn!(error = %e, "beacon: notify answer failed");
            metadata.response_code = ResponseCode::ServFail;
            failed(metadata)
        }
    }
}
