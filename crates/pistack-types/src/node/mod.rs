// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Node identity: one stable name per machine.
//!
//! The agent enrolls, says hello, heartbeats, and polls under this name.
//! The control plane keys host records by it, loom stamps it as
//! `pistack.io/hostname`, and the server hands it to modules as
//! `ModuleContext::node_name`. A server and an agent that share a data
//! directory share the name.
//!
//! The name is picked once and kept in `<data_dir>/node.name`. A restart,
//! a host rename, or a re-enroll keeps it. Order:
//!
//! 1. `PISTACK_NODE_NAME`, when set. It is written back to the file, so
//!    it is also how an operator renames a node on purpose.
//! 2. `<data_dir>/node.name`, when present.
//! 3. A name to adopt from earlier state (the agent passes the common name
//!    of a certificate it already enrolled with).
//! 4. The host name.
//! 5. `node-<12 hex>`, freshly minted, when there is no usable host name.
//!
//! The raft member id in `<data_dir>/node.id` is a separate, internal id
//! for cluster membership; it never reaches agents or hosts.

mod is_valid_node_name;
mod max_node_name;
mod node_identity;
mod node_identity_error;
mod node_name_env;
mod node_name_file;
mod node_name_source;
mod resolve_node_name;
mod resolve_node_name_with;
mod sanitize_node_name;
mod system_hostname;
#[cfg(test)]
mod tests;

pub use is_valid_node_name::is_valid_node_name;
pub use max_node_name::MAX_NODE_NAME;
pub use node_identity::NodeIdentity;
pub use node_identity_error::NodeIdentityError;
pub use node_name_env::NODE_NAME_ENV;
pub use node_name_file::NODE_NAME_FILE;
pub use node_name_source::NodeNameSource;
pub use resolve_node_name::resolve_node_name;
pub use resolve_node_name_with::resolve_node_name_with;
pub use sanitize_node_name::sanitize_node_name;
pub use system_hostname::system_hostname;
