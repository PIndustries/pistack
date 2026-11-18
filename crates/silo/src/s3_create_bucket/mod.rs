// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3 CreateBucket: `PUT /{bucket}` on the path-style surface.
//!
//! Makes the same bucket record the native `POST /buckets` makes, for the
//! signed caller's project, through the same `Client::create_bucket` (name
//! rules and quota included). The `CreateBucketConfiguration` body is read to
//! honour the declared payload hash, then ignored.

mod create_bucket;
mod create_bucket_inner;
mod location;
mod same_project;
mod taken;
pub use create_bucket::create_bucket;
