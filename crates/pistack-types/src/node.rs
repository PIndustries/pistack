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

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Overrides the node name, and is persisted when set.
pub const NODE_NAME_ENV: &str = "PISTACK_NODE_NAME";
/// File under the data directory that keeps the node name.
pub const NODE_NAME_FILE: &str = "node.name";
/// Longest accepted node name (a DNS name).
pub const MAX_NODE_NAME: usize = 253;

/// Where a resolved node name came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeNameSource {
    Env,
    File,
    Adopted,
    Hostname,
    Minted,
}

impl NodeNameSource {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeNameSource::Env => "env",
            NodeNameSource::File => "file",
            NodeNameSource::Adopted => "adopted",
            NodeNameSource::Hostname => "hostname",
            NodeNameSource::Minted => "minted",
        }
    }
}

impl fmt::Display for NodeNameSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The node name and how it was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeIdentity {
    pub name: String,
    pub source: NodeNameSource,
    /// The name the file held before `PISTACK_NODE_NAME` replaced it.
    pub replaced: Option<String>,
}

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

/// 1..=253 ASCII letters, digits, `-`, `.` and `_`. The agent protocol
/// rejects anything else.
pub fn is_valid_node_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NODE_NAME
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

/// Turn a host name into a node name: other characters become `-`, the
/// result is cut to [`MAX_NODE_NAME`]. Empty input and `localhost`, which
/// every machine shares, give `None`.
pub fn sanitize_node_name(raw: &str) -> Option<String> {
    let mapped: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_') {
                c
            } else {
                '-'
            }
        })
        .take(MAX_NODE_NAME)
        .collect();
    let name = mapped.trim_matches(|c| c == '-' || c == '.');
    if name.is_empty()
        || name.eq_ignore_ascii_case("localhost")
        || name.eq_ignore_ascii_case("localhost.localdomain")
    {
        None
    } else {
        Some(name.to_owned())
    }
}

/// This machine's host name, made safe as a node name.
pub fn system_hostname() -> Option<String> {
    for path in ["/etc/hostname", "/proc/sys/kernel/hostname"] {
        if let Some(name) = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| sanitize_node_name(&s))
        {
            return Some(name);
        }
    }
    if let Some(name) = gethostname().and_then(|s| sanitize_node_name(&s)) {
        return Some(name);
    }
    std::env::var("HOSTNAME")
        .ok()
        .and_then(|s| sanitize_node_name(&s))
}

#[cfg(unix)]
fn gethostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for `buf.len()` bytes, and the result is
    // read only up to the first NUL inside it.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    std::str::from_utf8(&buf[..end]).ok().map(str::to_owned)
}

#[cfg(not(unix))]
fn gethostname() -> Option<String> {
    None
}

/// Resolve and persist this node's name from the process environment and
/// the host name.
pub fn resolve_node_name(data_dir: &Path) -> Result<NodeIdentity, NodeIdentityError> {
    let env = std::env::var(NODE_NAME_ENV).ok();
    resolve_node_name_with(data_dir, env.as_deref(), None, system_hostname)
}

/// Like [`resolve_node_name`] with every input explicit. `adopt` is a name
/// from earlier state used when the file is missing. `hostname` is called
/// only when nothing earlier in the order applies.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &'static str) -> impl FnOnce() -> Option<String> {
        move || Some(name.to_owned())
    }

    fn stored(dir: &Path) -> String {
        std::fs::read_to_string(dir.join(NODE_NAME_FILE))
            .unwrap()
            .trim()
            .to_owned()
    }

    #[test]
    fn first_start_persists_the_host_name() {
        let dir = tempfile::tempdir().unwrap();
        let id = resolve_node_name_with(dir.path(), None, None, host("rack1-n3")).unwrap();
        assert_eq!(id.name, "rack1-n3");
        assert_eq!(id.source, NodeNameSource::Hostname);
        assert_eq!(id.replaced, None);
        assert_eq!(stored(dir.path()), "rack1-n3");
    }

    #[test]
    fn restart_after_host_rename_keeps_the_name() {
        let dir = tempfile::tempdir().unwrap();
        resolve_node_name_with(dir.path(), None, None, host("rack1-n3")).unwrap();
        let id = resolve_node_name_with(dir.path(), None, None, || {
            panic!("host name must not be consulted once a name is stored")
        })
        .unwrap();
        assert_eq!(id.name, "rack1-n3");
        assert_eq!(id.source, NodeNameSource::File);
        let id = resolve_node_name_with(dir.path(), None, Some("other"), host("renamed")).unwrap();
        assert_eq!(id.name, "rack1-n3");
        assert_eq!(stored(dir.path()), "rack1-n3");
    }

    #[test]
    fn env_wins_and_sticks_after_it_is_unset() {
        let dir = tempfile::tempdir().unwrap();
        resolve_node_name_with(dir.path(), None, None, host("old")).unwrap();
        let id = resolve_node_name_with(dir.path(), Some(" lab-sim3 "), None, host("old")).unwrap();
        assert_eq!(id.name, "lab-sim3");
        assert_eq!(id.source, NodeNameSource::Env);
        assert_eq!(id.replaced.as_deref(), Some("old"));
        let id = resolve_node_name_with(dir.path(), None, None, host("old")).unwrap();
        assert_eq!(id.name, "lab-sim3");
        assert_eq!(id.source, NodeNameSource::File);
        assert_eq!(id.replaced, None);
    }

    #[test]
    fn blank_env_is_unset() {
        let dir = tempfile::tempdir().unwrap();
        let id = resolve_node_name_with(dir.path(), Some("  "), None, host("h1")).unwrap();
        assert_eq!(id.name, "h1");
        assert_eq!(id.source, NodeNameSource::Hostname);
    }

    #[test]
    fn invalid_env_is_an_error_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let err =
            resolve_node_name_with(dir.path(), Some("bad name"), None, host("h1")).unwrap_err();
        assert!(matches!(err, NodeIdentityError::InvalidEnv(_)), "{err}");
        assert!(!dir.path().join(NODE_NAME_FILE).exists());
    }

    #[test]
    fn invalid_file_is_an_error_not_a_silent_rename() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(NODE_NAME_FILE), "no/slashes\n").unwrap();
        let err = resolve_node_name_with(dir.path(), None, None, host("h1")).unwrap_err();
        assert!(
            matches!(err, NodeIdentityError::InvalidFile { .. }),
            "{err}"
        );
    }

    #[test]
    fn adopted_name_beats_the_host_name_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let id = resolve_node_name_with(dir.path(), None, Some("enrolled-as"), host("h1")).unwrap();
        assert_eq!(id.name, "enrolled-as");
        assert_eq!(id.source, NodeNameSource::Adopted);
        assert_eq!(stored(dir.path()), "enrolled-as");
    }

    #[test]
    fn no_host_name_mints_once() {
        let dir = tempfile::tempdir().unwrap();
        let id = resolve_node_name_with(dir.path(), None, None, || None).unwrap();
        assert_eq!(id.source, NodeNameSource::Minted);
        assert!(
            id.name.starts_with("node-") && id.name.len() == 17,
            "{}",
            id.name
        );
        assert!(is_valid_node_name(&id.name));
        let again = resolve_node_name_with(dir.path(), None, None, || None).unwrap();
        assert_eq!(again.name, id.name);
        assert_eq!(again.source, NodeNameSource::File);
    }

    #[test]
    fn missing_data_dir_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        let id = resolve_node_name_with(&nested, None, None, host("h1")).unwrap();
        assert_eq!(id.name, "h1");
        assert_eq!(stored(&nested), "h1");
    }

    #[test]
    fn sanitize_host_names() {
        assert_eq!(sanitize_node_name(" n1\n").as_deref(), Some("n1"));
        assert_eq!(
            sanitize_node_name("Dave's MacBook Pro.local").as_deref(),
            Some("Dave-s-MacBook-Pro.local")
        );
        assert_eq!(sanitize_node_name(""), None);
        assert_eq!(sanitize_node_name("localhost"), None);
        assert_eq!(sanitize_node_name("--"), None);
        let long = "a".repeat(400);
        assert_eq!(sanitize_node_name(&long).unwrap().len(), MAX_NODE_NAME);
    }

    #[test]
    fn validity_matches_the_agent_protocol() {
        assert!(is_valid_node_name("host-a.rack_1"));
        assert!(!is_valid_node_name(""));
        assert!(!is_valid_node_name("a b"));
        assert!(!is_valid_node_name("a/b"));
        assert!(!is_valid_node_name(&"a".repeat(254)));
        assert!(is_valid_node_name(&"a".repeat(253)));
    }
}
