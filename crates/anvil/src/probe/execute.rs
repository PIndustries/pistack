// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::due_check::DueCheck;
use crate::probe::book::outcome::Outcome;
use crate::probe::exec::command_probe_reply::CommandProbeReply;
use crate::probe::run::run;

/// Run one check when no runtime is available. An exec probe is unsupported,
/// so it cannot become a pass.
#[cfg(test)]
pub async fn execute(check: &DueCheck) -> Outcome {
    run(check, |_, _| async { CommandProbeReply::Unsupported }).await
}
