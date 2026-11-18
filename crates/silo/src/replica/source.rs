// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

#[derive(Clone)]
pub(crate) enum Source {
    /// Read the member list from the store each time.
    Cluster { store: Store, me: Option<String> },
    /// A fixed list of API base URLs.
    Fixed(Vec<String>),
}
