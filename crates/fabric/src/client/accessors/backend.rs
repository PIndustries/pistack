// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::backend::NetBackend;
use crate::client::Client;
use std::sync::Arc;
impl Client {
    pub fn backend(&self) -> &Arc<dyn NetBackend> {
        &self.backend
    }
}
