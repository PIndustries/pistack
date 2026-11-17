// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use std::sync::Arc;

pub(crate) fn fabric_client(store: &Store) -> fabric::Client {
    fabric::Client::new(store.clone(), Arc::new(fabric::MockBackend::new()))
}
