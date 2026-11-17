// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `resolve_node_name`: resolve and persist this node's name from the
//! environment and the host name.

use super::node_identity::NodeIdentity;
use super::node_identity_error::NodeIdentityError;
use super::node_name_env::NODE_NAME_ENV;
use super::resolve_node_name_with::resolve_node_name_with;
use super::system_hostname::system_hostname;
use std::path::Path;

/// Resolve and persist this node's name from the process environment and
/// the host name.
pub fn resolve_node_name(data_dir: &Path) -> Result<NodeIdentity, NodeIdentityError> {
    let env = std::env::var(NODE_NAME_ENV).ok();
    resolve_node_name_with(data_dir, env.as_deref(), None, system_hostname)
}
