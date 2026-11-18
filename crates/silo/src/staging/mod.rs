// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Multipart staging on more than one control-plane server.
//!
//! An unfinished multipart upload is files on disk, not store records
//! (see [`crate::multipart`]), so nothing replicates it. Without this
//! module an upload in flight when the leader changes is gone: the new
//! leader answers `NoSuchUpload` and the client starts again.
//!
//! * **Copy before the answer.** CreateMultipartUpload and UploadPart are
//!   answered only after one other server holds the upload's `meta.json`
//!   or the part, the same rule and the same `put_copies` setting as a
//!   put. The server that took the request calls
//!   `POST /silo/peer/uploads/{bucket-id}/{upload-id}[/{part}]` on each
//!   peer in turn; the peer fetches from the others and checks the part's
//!   length and MD5 before it answers.
//! * **Background pull.** [`sync`] runs beside the object pull on every
//!   server. It reads `GET /silo/peer/uploads` from each peer and takes
//!   the uploads and parts it lacks, so a third server catches up and a
//!   server that was down gets what it missed.
//! * **Finished uploads.** Complete and abort leave a small file,
//!   `{bucket}/.uploads-done/{upload-id}`. A server that has it never
//!   takes that upload again, and one that sees it in a peer's list
//!   removes its own copy. Without it a finished upload would be copied
//!   back from a server that had not heard yet. [`crate::gc`] removes the
//!   file once it is twice as old as an upload may get.
//!
//! A part can be uploaded again under the same number. When two servers
//! hold different bytes for a part, the one written later wins: the list
//! carries each part's modification time and a fetched part keeps it.

mod client_staging;
mod done_dir;
mod done_upload;
mod download;
mod modified_ms;
mod now_ms;
mod read_parts;
mod staged;
mod staged_part;
mod staged_report;
mod staged_upload;
mod stall;
mod sweep_done;
mod sync;
mod want;
mod write_atomic;
pub use done_upload::DoneUpload;
pub use staged::Staged;
pub use staged_part::StagedPart;
pub use staged_report::StagedReport;
pub use staged_upload::StagedUpload;
pub(crate) use sweep_done::sweep_done;
pub use sync::sync;
pub use want::Want;
