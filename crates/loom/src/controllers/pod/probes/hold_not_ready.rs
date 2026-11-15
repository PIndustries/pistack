// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::types::{ContainerState, ContainerStatus};
pub(in crate::controllers::pod) fn hold_not_ready(
    cs: &mut ContainerStatus,
    message: &str,
    waiting: &mut Vec<String>,
) {
    cs.ready = false;
    cs.state = ContainerState::Waiting {
        reason: message.to_owned(),
    };
    waiting.push(format!("{}: {message}", cs.name));
}
