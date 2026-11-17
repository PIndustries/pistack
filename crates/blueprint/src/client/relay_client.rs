// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use std::sync::Arc;

pub(crate) fn relay_client(store: &Store) -> relay::Client {
    relay::Client::new(
        store.clone(),
        relay::Providers::new(vec![Arc::new(relay::MockProvider::new())]),
    )
}
