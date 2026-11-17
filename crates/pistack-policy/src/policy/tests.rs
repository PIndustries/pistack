// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::policy_error::PolicyError;
use super::policy_eval::Policy;
use super::target::Target;
use pistack_types::{Caller, Id};

use pistack_types::kind;

fn caller(roles: &[&str], project: Option<&Id>, system: bool) -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "alice".into(),
        project: project.cloned(),
        roles: roles.iter().map(|r| (*r).to_string()).collect(),
        system,
    }
}

#[test]
fn default_policy_is_valid_and_covers_crud() {
    let p = Policy::default_policy();
    for kind in [
        "machine", "volume", "network", "project", "host", "image", "token", "pod",
    ] {
        for verb in ["create", "read", "list", "update", "delete"] {
            assert!(
                p.rule_for(&format!("{kind}:{verb}")).is_some(),
                "{kind}:{verb}"
            );
        }
    }
    // Document form round-trips.
    let doc = p.to_document();
    let text = serde_yaml_ng::to_string(&doc).unwrap();
    let again = Policy::parse(&text).unwrap();
    assert_eq!(again, p);
}

#[test]
fn admin_can_do_everything() {
    let p = Policy::default_policy();
    let other = Id::new(kind::PROJECT);
    let admin = caller(&["admin"], None, false);
    let sys = caller(&[], None, true);
    for (action, _) in p.actions() {
        if action == "token:update" {
            continue; // nobody may mutate a token
        }
        let t = Target::project(other.clone());
        assert!(p.is_allowed(&admin, action, &t), "admin denied {action}");
        assert!(p.is_allowed(&sys, action, &t), "system denied {action}");
    }
}

#[test]
fn member_rw_in_own_project_only() {
    let p = Policy::default_policy();
    let mine = Id::new(kind::PROJECT);
    let theirs = Id::new(kind::PROJECT);
    let m = caller(&["member"], Some(&mine), false);

    assert!(p
        .authorize(&m, "machine:create", &Target::project(mine.clone()))
        .is_ok());
    assert!(p
        .authorize(&m, "machine:delete", &Target::project(mine.clone()))
        .is_ok());
    assert!(p
        .authorize(&m, "machine:read", &Target::project(mine.clone()))
        .is_ok());

    let err = p
        .authorize(&m, "machine:create", &Target::project(theirs.clone()))
        .unwrap_err();
    assert_eq!(err.code, pistack_types::error::ErrorCode::Forbidden);
    assert_eq!(err.message, "alice is not allowed to machine:create");
    assert_eq!(
        err.details.get("action").and_then(|v| v.as_str()),
        Some("machine:create")
    );
    assert!(p
        .authorize(&m, "machine:read", &Target::project(theirs))
        .is_err());

    // Cloud-scoped things are off limits.
    assert!(p.authorize(&m, "host:list", &Target::cloud()).is_err());
    assert!(p.authorize(&m, "project:create", &Target::cloud()).is_err());
    // But flavors are readable by anyone.
    assert!(p.authorize(&m, "flavor:list", &Target::cloud()).is_ok());
    assert!(p.authorize(&m, "flavor:create", &Target::cloud()).is_err());
}

#[test]
fn reader_is_read_only() {
    let p = Policy::default_policy();
    let mine = Id::new(kind::PROJECT);
    let r = caller(&["reader"], Some(&mine), false);
    let t = Target::project(mine.clone());
    assert!(p.is_allowed(&r, "volume:read", &t));
    assert!(p.is_allowed(&r, "volume:list", &t));
    assert!(!p.is_allowed(&r, "volume:create", &t));
    assert!(!p.is_allowed(&r, "volume:delete", &t));
    assert!(!p.is_allowed(&r, "machine:start", &t));
}

#[test]
fn project_member_requires_matching_scope() {
    let p = Policy::parse("actions:\n  x:do: project_member\n").unwrap();
    let proj = Id::new(kind::PROJECT);
    // Unscoped caller never matches, even against an unscoped target.
    let unscoped = caller(&["member"], None, false);
    assert!(!p.is_allowed(&unscoped, "x:do", &Target::cloud()));
    // Scoped caller against a cloud target: no.
    let scoped = caller(&["member"], Some(&proj), false);
    assert!(!p.is_allowed(&scoped, "x:do", &Target::cloud()));
    assert!(p.is_allowed(&scoped, "x:do", &Target::project(proj)));
}

#[test]
fn owner_rule() {
    let p = Policy::default_policy();
    let me = caller(&[], None, false);
    let someone = Id::new(kind::PRINCIPAL);
    assert!(p.is_allowed(
        &me,
        "token:delete",
        &Target::cloud().with_owner(me.principal.clone())
    ));
    assert!(!p.is_allowed(&me, "token:delete", &Target::cloud().with_owner(someone)));
    assert!(!p.is_allowed(&me, "token:delete", &Target::cloud()));
    assert!(p.is_allowed(&me, "token:create", &Target::cloud()));
    assert!(!p.is_allowed(&me, "token:update", &Target::cloud()));
}

#[test]
fn unknown_action_is_denied() {
    let p = Policy::default_policy();
    let admin = caller(&["admin"], None, true);
    assert!(!p.is_allowed(&admin, "teleporter:engage", &Target::cloud()));
    let err = p
        .authorize(&admin, "teleporter:engage", &Target::cloud())
        .unwrap_err();
    assert_eq!(err.code, pistack_types::error::ErrorCode::Forbidden);
}

#[test]
fn parse_yaml_and_json() {
    let yaml = r#"
rules:
  ops: "role:ops or system"
actions:
  "machine:create": "rule:ops"
  "machine:read": true
"#;
    let p = Policy::parse(yaml).unwrap();
    let ops = caller(&["ops"], None, false);
    assert!(p.is_allowed(&ops, "machine:create", &Target::cloud()));
    assert!(p.is_allowed(&caller(&[], None, false), "machine:read", &Target::cloud()));

    let json = r#"{"rules":{"ops":"role:ops"},"actions":{"machine:create":"rule:ops"}}"#;
    let p2 = Policy::parse(json).unwrap();
    assert!(p2.is_allowed(&ops, "machine:create", &Target::cloud()));
    assert!(!p2.is_allowed(&caller(&[], None, true), "machine:create", &Target::cloud()));
}

#[test]
fn parse_errors_are_specific() {
    let e = Policy::parse("actions:\n  \"machine:create\": \"role:admin or\"\n").unwrap_err();
    assert_eq!(
        e.to_string(),
        "action 'machine:create': column 14: unexpected end of expression; expected a term"
    );

    let e = Policy::parse("rules:\n  bad: \"owner system\"\n").unwrap_err();
    assert_eq!(
        e.to_string(),
        "rule 'bad': column 7: unexpected 'system' after complete expression"
    );

    let e = Policy::parse("actions:\n  \"x:y\": \"rule:nope\"\n").unwrap_err();
    assert_eq!(
        e,
        PolicyError::UnknownRule {
            referrer: "action 'x:y'".into(),
            rule: "nope".into()
        }
    );
    assert_eq!(e.to_string(), "action 'x:y' references unknown rule 'nope'");

    let e = Policy::parse("rules:\n  a: \"rule:b\"\n").unwrap_err();
    assert_eq!(e.to_string(), "rule 'a' references unknown rule 'b'");

    let e = Policy::parse("rules: [1, 2]\n").unwrap_err();
    assert!(matches!(e, PolicyError::Document(_)), "{e}");

    let e = Policy::parse("actions:\n  x: 42\n").unwrap_err();
    assert!(matches!(e, PolicyError::Document(_)), "{e}");
}

#[test]
fn rule_cycles_are_rejected() {
    let e = Policy::parse("rules:\n  a: \"rule:b\"\n  b: \"rule:c\"\n  c: \"rule:a or owner\"\n")
        .unwrap_err();
    match e {
        PolicyError::Cycle { path } => {
            assert_eq!(path, vec!["a", "b", "c", "a"]);
        }
        other => panic!("expected cycle, got {other}"),
    }
    let e = Policy::parse("rules:\n  me: \"not rule:me\"\n").unwrap_err();
    assert_eq!(e.to_string(), "rule reference cycle: me -> me");
}

#[test]
fn merge_overrides_and_keeps_rest() {
    let mut p = Policy::default_policy();
    let mine = Id::new(kind::PROJECT);
    let reader = caller(&["reader"], Some(&mine), false);
    let t = Target::project(mine.clone());
    assert!(!p.is_allowed(&reader, "machine:create", &t));

    // Let readers create machines; also tighten snapshot deletion to admins.
    p.merge_text(
        "actions:\n  \"machine:create\": \"rule:reader\"\n  \"snapshot:delete\": \"rule:admin\"\n",
    )
    .unwrap();
    assert!(p.is_allowed(&reader, "machine:create", &t));
    let member = caller(&["member"], Some(&mine), false);
    assert!(!p.is_allowed(&member, "snapshot:delete", &t));
    // Untouched actions still work.
    assert!(p.is_allowed(&member, "volume:create", &t));

    // Override may reference a base rule and may add a new action.
    p.merge_text("actions:\n  \"teleporter:engage\": \"rule:admin\"\n")
        .unwrap();
    assert!(p.is_allowed(&caller(&["admin"], None, false), "teleporter:engage", &t));
}

#[test]
fn merge_redefining_a_rule_changes_dependents() {
    let mut p = Policy::default_policy();
    let mine = Id::new(kind::PROJECT);
    let ops = caller(&["ops"], None, false);
    assert!(!p.is_allowed(&ops, "host:list", &Target::cloud()));
    p.merge_text("rules:\n  admin: \"role:admin or role:ops or system\"\n")
        .unwrap();
    assert!(p.is_allowed(&ops, "host:list", &Target::cloud()));
    assert!(p.is_allowed(&ops, "machine:create", &Target::project(mine)));
}

#[test]
fn failed_merge_leaves_policy_unchanged() {
    let mut p = Policy::default_policy();
    let before = p.clone();
    let e = p
        .merge_text("actions:\n  \"machine:create\": \"rule:ghost\"\n")
        .unwrap_err();
    assert_eq!(
        e.to_string(),
        "action 'machine:create' references unknown rule 'ghost'"
    );
    assert_eq!(p, before);

    // Introducing a cycle via override is also rejected.
    let e = p
        .merge_text("rules:\n  admin: \"rule:member\"\n")
        .unwrap_err();
    assert!(matches!(e, PolicyError::Cycle { .. }), "{e}");
    assert_eq!(p, before);
}

#[test]
fn empty_policy_denies_all() {
    let p = Policy::empty();
    assert!(!p.is_allowed(
        &caller(&["admin"], None, true),
        "machine:read",
        &Target::cloud()
    ));
    assert_eq!(p.actions().count(), 0);
}
