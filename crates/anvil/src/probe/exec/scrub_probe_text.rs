// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::consts::PROBE_MESSAGE_CAP;
use crate::probe::exec::redact_probe_secrets::redact_probe_secrets;
use crate::probe::exec::secret_probe_line::secret_probe_line;

pub(in crate::probe) fn scrub_probe_text(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.iter().copied().any(secret_probe_line) {
        return String::new();
    }
    let flat = lines
        .iter()
        .map(|line| redact_probe_secrets(line.trim()))
        .collect::<Vec<_>>()
        .join(" ");
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(PROBE_MESSAGE_CAP).collect()
}
