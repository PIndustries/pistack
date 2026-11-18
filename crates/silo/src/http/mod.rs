// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! HTTP surface, mounted by the gateway at `/silo`.
//!
//! ```text
//! GET  /                              module info
//! GET|POST        /buckets
//! GET|DELETE      /buckets/{id}
//! GET             /buckets/{id}/objects
//! PUT|GET|DELETE  /buckets/{id}/objects/{*key}
//!
//! S3 path-style (same router, same auth):
//! PUT|GET|HEAD|DELETE|POST  /{bucket}/{*key}  (POST: multipart create/complete)
//! GET                       /{bucket}?list-type=2&…
//! GET                       /{bucket}?uploads&…  (ListMultipartUploads)
//! GET                       /{bucket}?versions&…  (ListObjectVersions)
//! GET|PUT                   /{bucket}?versioning  (Get/PutBucketVersioning)
//! POST                      /{bucket}?delete     (DeleteObjects)
//!
//! Between control-plane servers (join token instead of a caller):
//! GET  /peer/blobs/{bucket-id}/{sha256}   bytes this server holds
//! ```
//!
//! `/peer/blobs/…` is matched before the S3 routes, so a bucket named
//! `peer` cannot serve keys under `blobs/` on the S3 path. The native
//! `/buckets/{id}/objects/…` routes still reach them.
//!
//! `{id}` is a bucket id or name. Object puts stream the body to disk and
//! refuse anything larger than the client's configured max (default 5 GiB).
//! `{*key}` is the object key and may contain `/`; `..`, empty, and absolute
//! keys are rejected. Native PUT also rejects XML-unsafe key characters
//! (same rule as S3 PutObject) after bucket visibility.
//!
//! The `Caller` comes from request extensions (gate bearer token) or from
//! AWS SigV4 verification ([`crate::sigv4`]) on S3 routes. Without one, the
//! request is rejected.

mod app_state;
mod authorize_peer;
mod bucket_id;
mod caller;
mod client_from_ref;
mod content_type_header;
mod create_bucket;
mod create_project;
mod default_limit;
mod delete_bucket;
mod delete_object;
mod get_bucket;
mod get_object;
mod info;
mod list_buckets;
mod list_objects;
mod list_project;
mod listed_object;
mod load_bucket;
mod load_copy_source_bucket;
mod max_limit;
mod object_list;
mod page;
mod peer_allowed;
mod peer_blob;
mod peer_part;
mod peer_part_pull;
mod peer_pull;
mod peer_upload;
mod peer_upload_pull;
mod peer_uploads;
mod put_object;
mod require_caller;
mod router;
mod router_with;
mod sig_v4_state_from_ref;
mod visible;
pub use app_state::AppState;
pub use router::router;
pub use router_with::router_with;
#[cfg(test)]
mod peer_auth_tests;
pub(crate) use content_type_header::content_type_header;
pub(crate) use create_project::create_project;
pub(crate) use load_bucket::load_bucket;
pub(crate) use load_copy_source_bucket::load_copy_source_bucket;
