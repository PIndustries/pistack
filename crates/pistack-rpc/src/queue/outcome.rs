// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::error::RpcError;
use crate::proto::CommandResult;

pub(crate) type Outcome = Result<CommandResult, RpcError>;
