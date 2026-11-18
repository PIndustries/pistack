// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::ListObjectEntry;

/// One row of a ListObjectVersions page: a kept generation, a prefix rolled up
/// by the delimiter, or a delete marker.
pub type VersionPage = (Vec<ListObjectEntry>, bool);
