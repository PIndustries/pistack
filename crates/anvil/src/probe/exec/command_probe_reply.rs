// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// What a runtime did with one command probe.
///
/// `Absent` means this runtime does not hold the machine. `Unsupported`
/// means it cannot run a command inside a container. Neither is a pass,
/// and neither counts as a failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CommandProbeReply {
    Absent,
    Unsupported,
    Finished { exit_code: i32, output: String },
    TimedOut,
    Failed { message: String },
}
