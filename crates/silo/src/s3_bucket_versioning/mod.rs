// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! S3 bucket versioning configuration: `PUT /{bucket}?versioning` and
//! `GET /{bucket}?versioning`.
//!
//! The setting is the one the store already has — `BucketSpec.versioning` —
//! written through [`Client::set_versioning`]. `Enabled` turns it on,
//! `Suspended` turns it off: the store keeps no third state, so a suspended
//! bucket behaves as it did before versioning was ever set, which is what
//! turning it off means here.
//!
//! A GET answers `<VersioningConfiguration/>` with no `<Status>` element when
//! versioning was never set, and with `<Status>Enabled</Status>` once the
//! bucket keeps generations.
//!
//! Both handlers are reached from the existing bucket routes — `GET
//! /{bucket}` and `PUT /{bucket}` notice the `versioning` query and hand over
//! — because axum will not register two routes for one path.

mod get_versioning;
mod parse_status;
mod put_handler;
mod put_versioning;
mod xmlns;
pub(crate) use get_versioning::get_versioning;
pub(crate) use put_handler::put_handler;
