// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::consts::MAX_CHUNK_BYTES;
use crate::chunked::consts::MAX_CHUNK_HEADER_BYTES;
use crate::chunked::decoded_stream::decoded_stream::DecodedStream;
use crate::chunked::decoded_stream::phase::Phase;
use crate::chunked::finish_trailer::finish_trailer;
use crate::chunked::parse_chunk_header::parse_chunk_header;
use crate::chunked::take_line_capped::take_line_capped;
use crate::multipart::s3_err;
use bytes::Bytes;
use pistack_types::ApiError;
use std::task::Poll;

impl DecodedStream {
    /// `Ready(None)` = need more wire bytes.
    pub(in crate::chunked) fn pump(&mut self) -> Poll<Option<Result<(), ApiError>>> {
        match &mut self.phase {
            Phase::Done => {
                self.finished = true;
                Poll::Ready(Some(Ok(())))
            }
            Phase::ExpectEof => {
                if !self.buf.is_empty() {
                    return Poll::Ready(Some(Err(s3_err(
                        "IncompleteBody",
                        "unexpected bytes after aws-chunked final chunk",
                    ))));
                }
                // Wait for body EOF (or more bytes → rejected in poll_next).
                Poll::Ready(None)
            }
            Phase::NeedHeader => {
                match take_line_capped(
                    &mut self.buf,
                    &mut self.scan_from,
                    MAX_CHUNK_HEADER_BYTES,
                    "aws-chunked chunk header line too long",
                ) {
                    Err(e) => Poll::Ready(Some(Err(e))),
                    Ok(None) => Poll::Ready(None),
                    Ok(Some(line)) => {
                        match parse_chunk_header(&line, self.auth.requires_chunk_signatures()) {
                            Err(e) => Poll::Ready(Some(Err(e))),
                            Ok((size, sig)) => {
                                if size > MAX_CHUNK_BYTES {
                                    return Poll::Ready(Some(Err(s3_err(
                                        "InvalidRequest",
                                        format!(
                                            "aws-chunked chunk size {size} exceeds limit {}",
                                            MAX_CHUNK_BYTES
                                        ),
                                    ))));
                                }
                                self.phase = Phase::NeedData {
                                    size,
                                    declared_sig: sig,
                                    chunk_buf: Vec::with_capacity(size.min(64 * 1024)),
                                };
                                Poll::Ready(Some(Ok(())))
                            }
                        }
                    }
                }
            }
            Phase::NeedData {
                size,
                declared_sig,
                chunk_buf,
            } => {
                let size = *size;
                if size == 0 {
                    // Final chunk: optional empty-data CRLF (AWS signed style).
                    if self.buf.starts_with(b"\r\n") {
                        let _ = self.buf.split_to(2);
                        self.scan_from = 0;
                    } else if self.buf.is_empty() {
                        return Poll::Ready(None);
                    } else if self.auth.trailer_checksum().is_none() {
                        if self.buf.len() >= 2 {
                            return Poll::Ready(Some(Err(s3_err(
                                "InvalidRequest",
                                "aws-chunked final chunk missing trailing CRLF",
                            ))));
                        }
                        return Poll::Ready(None);
                    }

                    let declared = declared_sig.clone();
                    if let Err(e) = self.verify_chunk_sig(declared.as_deref(), b"") {
                        return Poll::Ready(Some(Err(e)));
                    }
                    if self.decoded != self.auth.decoded_content_length {
                        return Poll::Ready(Some(Err(s3_err(
                            "IncompleteBody",
                            format!(
                                "decoded length {} does not match x-amz-decoded-content-length {}",
                                self.decoded, self.auth.decoded_content_length
                            ),
                        ))));
                    }
                    self.scan_from = 0;
                    if self.auth.trailer_checksum().is_some() {
                        self.phase = Phase::NeedTrailer;
                        return Poll::Ready(Some(Ok(())));
                    }
                    if !self.buf.is_empty() {
                        return Poll::Ready(Some(Err(s3_err(
                            "IncompleteBody",
                            "unexpected bytes after aws-chunked final chunk",
                        ))));
                    }
                    self.phase = Phase::ExpectEof;
                    return Poll::Ready(Some(Ok(())));
                }

                let need_total = size + 2;
                let have = chunk_buf.len() + self.buf.len();
                if have < need_total {
                    let take = (size - chunk_buf.len()).min(self.buf.len());
                    if take > 0 {
                        chunk_buf.extend_from_slice(&self.buf.split_to(take));
                        self.scan_from = 0;
                    }
                    return Poll::Ready(None);
                }
                let remain = size - chunk_buf.len();
                if remain > 0 {
                    chunk_buf.extend_from_slice(&self.buf.split_to(remain));
                }
                if self.buf.len() < 2 {
                    return Poll::Ready(None);
                }
                if &self.buf[..2] != b"\r\n" {
                    return Poll::Ready(Some(Err(s3_err(
                        "InvalidRequest",
                        "aws-chunked chunk missing trailing CRLF",
                    ))));
                }
                let _ = self.buf.split_to(2);
                self.scan_from = 0;

                let data = std::mem::take(chunk_buf);
                let declared = declared_sig.clone();
                if let Err(e) = self.verify_chunk_sig(declared.as_deref(), &data) {
                    return Poll::Ready(Some(Err(e)));
                }

                let next = self.decoded.saturating_add(data.len() as u64);
                if next > self.auth.decoded_content_length {
                    return Poll::Ready(Some(Err(s3_err(
                        "IncompleteBody",
                        format!(
                            "decoded length exceeds x-amz-decoded-content-length {}",
                            self.auth.decoded_content_length
                        ),
                    ))));
                }
                self.decoded = next;
                if let Some(cs) = self.checksum.as_mut() {
                    cs.update(&data);
                }
                self.pending = Some(Bytes::from(data));
                self.phase = Phase::NeedHeader;
                Poll::Ready(Some(Ok(())))
            }
            Phase::NeedTrailer => match finish_trailer(self) {
                Err(e) => Poll::Ready(Some(Err(e))),
                Ok(true) => {
                    if !self.buf.is_empty() {
                        return Poll::Ready(Some(Err(s3_err(
                            "IncompleteBody",
                            "unexpected bytes after aws-chunked trailer",
                        ))));
                    }
                    self.phase = Phase::ExpectEof;
                    Poll::Ready(Some(Ok(())))
                }
                Ok(false) => Poll::Ready(None),
            },
        }
    }
}
