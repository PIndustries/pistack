// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3 DeleteBucket: `DELETE /{bucket}` (no `uploadId`) on the path-style
//! surface.
//!
//! Removes the same record the native `DELETE /buckets/{id}` removes, through
//! the same visibility check (`load_bucket`) and the same
//! `Client::delete_bucket`. The S3 difference is that a bucket holding
//! objects or unfinished multipart uploads is refused (`BucketNotEmpty`)
//! instead of taken down with its contents.

mod delete_bucket;
mod delete_bucket_inner;
pub use delete_bucket::delete_bucket;
