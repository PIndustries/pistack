// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

use crate::proto::{CommandId, CommandKind};

/// Errors from the command queue and the agent client.
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("command queue for node {node} is full")]
    QueueFull { node: String },
    #[error("command {0} is not pending")]
    UnknownCommand(CommandId),
    #[error("command {id} ({kind}) to node {node} timed out")]
    Timeout {
        id: CommandId,
        kind: CommandKind,
        node: String,
    },
    #[error("node {node} was removed while command {id} was pending")]
    NodeRemoved { id: CommandId, node: String },
    #[error("command queue was dropped")]
    Closed,
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server replied {status}: {error}")]
    Api { status: u16, error: ApiError },
}

impl From<RpcError> for ApiError {
    fn from(e: RpcError) -> Self {
        match e {
            RpcError::QueueFull { .. } => ApiError::unavailable(e.to_string()),
            RpcError::UnknownCommand(_) => ApiError::not_found(e.to_string()),
            RpcError::Timeout { .. } => ApiError::unavailable(e.to_string()),
            RpcError::NodeRemoved { .. } => ApiError::unavailable(e.to_string()),
            RpcError::Closed => ApiError::internal(e.to_string()),
            RpcError::InvalidUrl(_) => ApiError::bad_request(e.to_string()),
            RpcError::Http(_) => ApiError::unavailable(e.to_string()),
            RpcError::Api { error, .. } => error,
        }
    }
}
