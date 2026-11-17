// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The pool's request: its protocol, algorithm, persistence and health
//! monitor.

use super::algorithm::Algorithm;
use super::protocol::PoolProtocol;
use super::session_persistence::SessionPersistence;
use crate::types::default_true::default_true;
use crate::types::health_monitor::HealthMonitor;
use pistack_types::Id;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PoolSpec {
    pub balancer: Id,
    #[serde(default)]
    pub protocol: PoolProtocol,
    #[serde(default)]
    pub algorithm: Algorithm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_persistence: Option<SessionPersistence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_monitor: Option<HealthMonitor>,
    #[serde(default = "default_true")]
    pub admin_up: bool,
}
