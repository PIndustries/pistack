// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

use super::bucket_spec::BucketSpec;
use super::bucket_status::BucketStatus;

pub type Bucket = Resource<BucketSpec, BucketStatus>;
