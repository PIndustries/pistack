// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid cron expression: {0}")]
pub struct CronError(pub String);
