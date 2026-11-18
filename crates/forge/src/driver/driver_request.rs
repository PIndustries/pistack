// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;
use tokio::sync::oneshot;

use super::driver_command::DriverCommand;
use super::driver_reply::DriverReply;

/// A command plus the oneshot to answer it on.
#[derive(Debug)]
pub struct DriverRequest {
    pub command: DriverCommand,
    pub reply: oneshot::Sender<ApiResult<DriverReply>>,
}
