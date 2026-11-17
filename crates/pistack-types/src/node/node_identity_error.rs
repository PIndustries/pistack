// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `NodeIdentityError`: why a node name could not be resolved.

use std::io;
use std::path::PathBuf;

use super::node_name_env::NODE_NAME_ENV;

#[derive(Debug, thiserror::Error)]
pub enum NodeIdentityError {
    #[error(
        "{NODE_NAME_ENV}={0:?} is not a valid node name (1..=253 letters, digits, '-', '.' or '_')"
    )]
    InvalidEnv(String),
    #[error("{} holds {name:?}, which is not a valid node name; fix or remove the file", path.display())]
    InvalidFile { path: PathBuf, name: String },
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
}
