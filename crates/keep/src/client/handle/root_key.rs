// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::crypto::RootKey;

impl Client {
    pub(crate) fn root_key(&self) -> &RootKey {
        &self.root_key
    }
}
