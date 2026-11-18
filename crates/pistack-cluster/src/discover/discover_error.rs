// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DiscoverError {
    #[error("advertise address {0} is not a private unicast")]
    Address(String),
    #[error("prefix /{0} is too wide to scan")]
    Prefix(u8),
    #[error("discovery socket: {0}")]
    Io(String),
}
