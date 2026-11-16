// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use thiserror::Error;

/// Why [`plan`] refused a document.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlanError {
    #[error("{0}")]
    Ip(String),
    #[error("vni {0} is outside 1..=16777215")]
    Vni(u32),
    #[error("overlay mtu would be 0")]
    Mtu,
    #[error("{0} does not yield 12 hex digits")]
    Id(String),
    #[error("refusing device name {0}")]
    Name(String),
    #[error("mac {0} is not six hex octets")]
    Mac(String),
    #[error("{0}")]
    Invalid(String),
}
