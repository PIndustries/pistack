// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Backend-selection tests: the parse and choose rules, and that the
//! forward backend serves both planes.

use super::RelayBackend;
use crate::provider::{ForwardProvider, LbProvider, OvnProvider};
use crate::types::Provider;
use crate::Relay;

#[test]
fn backend_choice_is_explicit() {
    assert_eq!(RelayBackend::parse(None), Ok(RelayBackend::Mock));
    assert_eq!(RelayBackend::parse(Some("")), Ok(RelayBackend::Mock));
    assert_eq!(RelayBackend::parse(Some("mock")), Ok(RelayBackend::Mock));
    assert_eq!(
        RelayBackend::parse(Some("forward")),
        Ok(RelayBackend::Forward)
    );
    assert_eq!(
        RelayBackend::parse(Some(" Forward\n")),
        Ok(RelayBackend::Forward)
    );
    assert!(RelayBackend::parse(Some("fwd")).is_err());
    assert!(RelayBackend::parse(Some("ovn")).is_err());
}

#[test]
fn a_real_fabric_defaults_to_forward() {
    use RelayBackend::{Forward, Mock};
    assert_eq!(RelayBackend::choose(None, None), Ok(Mock));
    assert_eq!(RelayBackend::choose(None, Some("mock")), Ok(Mock));
    assert_eq!(RelayBackend::choose(None, Some("hostnat")), Ok(Forward));
    assert_eq!(RelayBackend::choose(Some(""), Some("overlay")), Ok(Forward));
    assert_eq!(RelayBackend::choose(None, Some("ovn")), Ok(Forward));
    assert_eq!(
        RelayBackend::choose(Some(" "), Some("overlay")),
        Ok(Forward)
    );
    // An explicit value always wins, including a typo.
    assert_eq!(
        RelayBackend::choose(Some("mock"), Some("hostnat")),
        Ok(Mock)
    );
    assert_eq!(RelayBackend::choose(Some("forward"), None), Ok(Forward));
    assert!(RelayBackend::choose(Some("fwd"), Some("hostnat")).is_err());
}

#[test]
fn forward_backend_serves_l4_and_http() {
    let relay = Relay::for_backend(RelayBackend::Forward);
    let names = relay.providers.names();
    assert!(names.contains(&ForwardProvider::new().name()), "{names:?}");
    assert!(!names.contains(&"mock"), "{names:?}");
}

#[test]
fn old_ovn_rows_read_as_forward() {
    let p: Provider = serde_json::from_str("\"ovn\"").unwrap();
    assert_eq!(p, Provider::Forward);
    assert_eq!(serde_json::to_string(&p).unwrap(), "\"forward\"");
    assert!(!OvnProvider::default().serves(Provider::Forward));
}
