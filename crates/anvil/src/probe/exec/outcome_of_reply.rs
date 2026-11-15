// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::outcome::Outcome;
use crate::probe::exec::command_probe_reply::CommandProbeReply;
use crate::probe::exec::exit_message::exit_message;
use crate::probe::exec::scrub_probe_text::scrub_probe_text;

pub(crate) fn outcome_of_reply(reply: CommandProbeReply) -> Outcome {
    match reply {
        CommandProbeReply::Absent | CommandProbeReply::Unsupported => {
            Outcome::unsupported("exec probes are not supported")
        }
        CommandProbeReply::Finished { exit_code: 0, .. } => Outcome::ok(),
        CommandProbeReply::Finished { exit_code, output } => {
            Outcome::fail(exit_message(exit_code, &output))
        }
        CommandProbeReply::TimedOut => Outcome::fail("timed out"),
        CommandProbeReply::Failed { message } => {
            let message = scrub_probe_text(&message);
            if message.is_empty() {
                Outcome::fail("exec probe failed")
            } else {
                Outcome::fail(message)
            }
        }
    }
}
