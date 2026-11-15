// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::decoded_stream::decoded_stream::DecodedStream;
use crate::chunked::decoded_stream::phase::Phase;
use crate::multipart::s3_err;
use bytes::Bytes;
use futures::Stream;
use pistack_types::ApiError;
use std::pin::Pin;
use std::task::{Context, Poll};

impl Stream for DecodedStream {
    type Item = Result<Bytes, ApiError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(b) = this.pending.take() {
            return Poll::Ready(Some(Ok(b)));
        }
        if this.finished {
            return Poll::Ready(None);
        }

        loop {
            match this.pump() {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => match this.body.as_mut().poll_next(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(None) => {
                        if matches!(this.phase, Phase::ExpectEof) {
                            this.phase = Phase::Done;
                            this.finished = true;
                            return Poll::Ready(None);
                        }
                        return Poll::Ready(Some(Err(this.on_wire_eof())));
                    }
                    Poll::Ready(Some(Ok(chunk))) => {
                        if matches!(this.phase, Phase::ExpectEof) && !chunk.is_empty() {
                            this.finished = true;
                            return Poll::Ready(Some(Err(s3_err(
                                "IncompleteBody",
                                "unexpected bytes after aws-chunked final chunk",
                            ))));
                        }
                        if !chunk.is_empty() {
                            this.buf.extend_from_slice(&chunk);
                        }
                        continue;
                    }
                    Poll::Ready(Some(Err(e))) => {
                        this.finished = true;
                        return Poll::Ready(Some(Err(e)));
                    }
                },
                Poll::Ready(Some(Ok(()))) => {
                    if let Some(b) = this.pending.take() {
                        return Poll::Ready(Some(Ok(b)));
                    }
                    if this.finished {
                        return Poll::Ready(None);
                    }
                    continue;
                }
                Poll::Ready(Some(Err(e))) => {
                    this.finished = true;
                    return Poll::Ready(Some(Err(e)));
                }
            }
        }
    }
}
