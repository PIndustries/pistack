// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiResult, Id};

use crate::types::Bucket;

pub fn count(store: &Store, project: &Id) -> ApiResult<u64> {
    Ok(store.collection::<Bucket>().count(Some(project))? as u64)
}
