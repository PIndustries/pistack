// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::exec::combine_output::combine_output;
use crate::probe::exec::command_probe_reply::CommandProbeReply;
use pistack_rpc::proto::ExecOutput;
use pistack_types::{error::ErrorCode, ApiError};

/// Map a youki exec result onto a command-probe reply. A missing machine is
/// absent, a timeout is a timeout, and `not_implemented` is unsupported.
/// Other errors are failures. The output excerpt is capped by the caller.
pub fn reply_from_exec(result: Result<ExecOutput, ApiError>) -> CommandProbeReply {
    match result {
        Ok(out) => CommandProbeReply::Finished {
            exit_code: out.exit_code,
            output: combine_output(&out.stdout, &out.stderr),
        },
        Err(err) if err.code == ErrorCode::NotImplemented => CommandProbeReply::Unsupported,
        Err(err) if err.code == ErrorCode::NotFound && err.message.contains("not on this host") => {
            CommandProbeReply::Absent
        }
        Err(err) if err.message.contains("timed out") => CommandProbeReply::TimedOut,
        Err(err) => CommandProbeReply::Failed {
            message: err.message,
        },
    }
}
