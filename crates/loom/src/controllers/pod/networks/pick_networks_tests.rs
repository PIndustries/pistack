// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::DEFAULT_NETWORK;
use crate::controllers::pod::networks::test_support::*;
use crate::controllers::pod::networks::*;
use pistack_store::Store;
use pistack_types::{kind, Id};

#[test]
fn named_networks_win() {
    let store = Store::in_memory().unwrap();
    let p = Id::new(kind::PROJECT);
    assert_eq!(
        pick_networks(&store, &pod(&p, &["labnet"])).unwrap(),
        vec!["labnet"]
    );
}
#[test]
fn default_by_name_then_the_only_internal_one() {
    let store = Store::in_memory().unwrap();
    let p = Id::new(kind::PROJECT);
    net(&store, "labext", Some(p.clone()), true);
    net(&store, "labnet", Some(p.clone()), false);
    net(&store, "theirs", Some(Id::new(kind::PROJECT)), false);
    assert_eq!(
        pick_networks(&store, &pod(&p, &[])).unwrap(),
        vec!["labnet"]
    );
    net(&store, DEFAULT_NETWORK, None, false);
    assert_eq!(
        pick_networks(&store, &pod(&p, &[])).unwrap(),
        vec![DEFAULT_NETWORK]
    );
}
#[test]
fn none_or_several_fail_clearly() {
    let store = Store::in_memory().unwrap();
    let p = Id::new(kind::PROJECT);
    net(&store, "labext", Some(p.clone()), true);
    let none = pick_networks(&store, &pod(&p, &[])).unwrap_err();
    assert!(none.contains("no network that is not external"), "{none}");
    net(&store, "labnet", Some(p.clone()), false);
    net(&store, "app", Some(p.clone()), false);
    let several = pick_networks(&store, &pod(&p, &[])).unwrap_err();
    assert!(several.contains("(app, labnet)"), "{several}");
    assert!(several.contains("spec.networks"), "{several}");
}
