// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `resolve_node_name_with`: resolve a node name with every input explicit.

use super::is_valid_node_name::is_valid_node_name;
use super::node_identity::NodeIdentity;
use super::node_identity_error::NodeIdentityError;
use super::node_name_file::NODE_NAME_FILE;
use super::node_name_source::NodeNameSource;
use std::io;
use std::path::Path;

/// Like [`resolve_node_name`](super::resolve_node_name::resolve_node_name)
/// with every input explicit. `adopt` is a name from earlier state used when
/// the file is missing. `hostname` is called only when nothing earlier in the
/// order applies.
pub fn resolve_node_name_with(
    data_dir: &Path,
    env: Option<&str>,
    adopt: Option<&str>,
    hostname: impl FnOnce() -> Option<String>,
) -> Result<NodeIdentity, NodeIdentityError> {
    let path = data_dir.join(NODE_NAME_FILE);
    let stored = read_stored(&path)?;

    let env = env.map(str::trim).filter(|s| !s.is_empty());
    let (name, source) = if let Some(name) = env {
        if !is_valid_node_name(name) {
            return Err(NodeIdentityError::InvalidEnv(name.to_owned()));
        }
        (name.to_owned(), NodeNameSource::Env)
    } else if let Some(name) = stored.clone() {
        (name, NodeNameSource::File)
    } else if let Some(name) = adopt.map(str::trim).filter(|s| is_valid_node_name(s)) {
        (name.to_owned(), NodeNameSource::Adopted)
    } else if let Some(name) = hostname().filter(|s| is_valid_node_name(s)) {
        (name, NodeNameSource::Hostname)
    } else {
        (mint(), NodeNameSource::Minted)
    };

    let replaced = stored.clone().filter(|old| *old != name);
    if stored.as_deref() != Some(name.as_str()) {
        write_stored(&path, &name)?;
    }
    Ok(NodeIdentity {
        name,
        source,
        replaced,
    })
}

fn read_stored(path: &Path) -> Result<Option<String>, NodeIdentityError> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let name = text.trim();
            if name.is_empty() {
                Ok(None)
            } else if is_valid_node_name(name) {
                Ok(Some(name.to_owned()))
            } else {
                Err(NodeIdentityError::InvalidFile {
                    path: path.to_owned(),
                    name: name.to_owned(),
                })
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(NodeIdentityError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

/// Write through a temporary file and rename, so a crash never leaves a
/// half-written name behind.
fn write_stored(path: &Path, name: &str) -> Result<(), NodeIdentityError> {
    let io_err = |source| NodeIdentityError::Io {
        path: path.to_owned(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_err)?;
    }
    let tmp = path.with_extension(format!("name.tmp.{}", std::process::id()));
    std::fs::write(&tmp, format!("{name}\n")).map_err(io_err)?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io_err(e)
    })
}

fn mint() -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("node-{}", &hex[..12])
}
