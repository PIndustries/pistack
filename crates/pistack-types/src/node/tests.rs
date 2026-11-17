// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::is_valid_node_name::is_valid_node_name;
use super::node_identity_error::NodeIdentityError;
use super::node_name_file::NODE_NAME_FILE;
use super::node_name_source::NodeNameSource;
use super::resolve_node_name_with::resolve_node_name_with;
use super::sanitize_node_name::sanitize_node_name;
use std::path::Path;

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
    let err = resolve_node_name_with(dir.path(), Some("bad name"), None, host("h1")).unwrap_err();
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
    assert_eq!(
        sanitize_node_name(&long).unwrap().len(),
        super::max_node_name::MAX_NODE_NAME
    );
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
