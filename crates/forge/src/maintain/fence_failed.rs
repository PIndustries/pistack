// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;

/// What a fence attempt that did not end in a confirmed power-off has to say,
/// with the reason the host could not be proved off in it.
pub(crate) fn fence_failed(host: &Host, transport: &str, reason: &str) -> String {
    format!(
        "a power-off over {transport} was tried for host {} and failed ({reason})",
        host.meta.name
    )
}
