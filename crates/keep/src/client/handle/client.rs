// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

use crate::ca::Ca;
use crate::crypto::RootKey;
use crate::quota::KeepConfig;

#[derive(Clone)]
pub struct Client {
    pub(in crate::client) store: Store,
    pub(in crate::client) root_key: RootKey,
    pub(in crate::client) ca: Ca,
    pub(in crate::client) limits: KeepConfig,
}
