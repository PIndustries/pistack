// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::client::Client;
use crate::types::*;
use pistack_store::Collection;
impl Client {
    pub fn security_groups(&self) -> Collection<SecurityGroup> {
        self.store.collection()
    }
}
