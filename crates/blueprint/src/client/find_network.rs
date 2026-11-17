// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{ApiResult, Id};

use super::find_named::find_named;

pub(crate) fn find_network(
    store: &Store,
    project: Option<&Id>,
    name_or_id: &str,
    subnet: &str,
) -> ApiResult<fabric::Network> {
    find_named(store, project, name_or_id, "subnet", subnet, "network")
}
