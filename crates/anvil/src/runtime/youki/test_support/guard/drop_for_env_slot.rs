// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::restore;
use super::EnvSlot;

impl Drop for EnvSlot {
    fn drop(&mut self) {
        restore(self.key, self.prev.take());
    }
}
