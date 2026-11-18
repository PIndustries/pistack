// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use hickory_proto::op::Message;

/// The apex a message asks about, absolute and lower-case, as the store keeps
/// zone names.
pub(crate) fn asked_apex(message: &Message) -> Option<String> {
    let mut name = message.queries.first()?.name().clone();
    name.set_fqdn(true);
    Some(name.to_ascii().to_ascii_lowercase())
}
