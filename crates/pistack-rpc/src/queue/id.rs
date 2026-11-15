// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::proto::CommandId;

use super::ticket::Ticket;

impl Ticket {
    pub fn id(&self) -> CommandId {
        self.id
    }
}
