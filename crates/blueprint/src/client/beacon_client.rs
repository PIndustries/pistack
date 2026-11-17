// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

pub(crate) fn beacon_client(store: &Store) -> beacon::Client {
    beacon::Client::new(store.clone())
}
