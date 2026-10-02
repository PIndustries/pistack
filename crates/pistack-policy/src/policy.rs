// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The policy document: named rules plus a map from action name to rule
//! expression, and the evaluator that answers "may this caller do this
//! action to this target?".
//!
//! Document shape (YAML or JSON; JSON is a subset of YAML):
//!
//! ```yaml
//! rules:
//!   admin:  "role:admin or system"
//!   member: "rule:admin or (project_member and role:member)"
//! actions:
//!   "machine:create": "rule:member"
//!   "machine:read":   "rule:reader"
//! ```
//!
//! Action names are `<kind>:<verb>` by convention. `gate` evaluates one
//! document for the whole cloud; operators override pieces of the default
//! with [`Policy::merge`].

use std::collections::{BTreeMap, BTreeSet};

use pistack_types::{ApiError, Caller, Id};
use serde::{Deserialize, Serialize};

use crate::expr::{self, Expr, ParseError};

/// The object an action is applied to. For `create` and `list`, `project`
/// is the project scope of the request and `owner` is usually `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    pub project: Option<Id>,
    pub owner: Option<Id>,
    pub labels: BTreeMap<String, String>,
}

impl Target {
    /// A cloud-scoped target (no project, no owner): hosts, flavors, the
    /// policy document itself.
    pub fn cloud() -> Self {
        Self::default()
    }
    /// A target inside `project`.
    pub fn project(project: Id) -> Self {
        Self {
            project: Some(project),
            ..Self::default()
        }
    }
    pub fn with_owner(mut self, owner: Id) -> Self {
        self.owner = Some(owner);
        self
    }
    pub fn with_label(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.labels.insert(k.into(), v.into());
        self
    }
}

/// Why a policy document was rejected.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error("policy document is not valid YAML/JSON: {0}")]
    Document(String),
    #[error("rule '{name}': {source}")]
    RuleSyntax { name: String, source: ParseError },
    #[error("action '{action}': {source}")]
    ActionSyntax { action: String, source: ParseError },
    #[error("{referrer} references unknown rule '{rule}'")]
    UnknownRule { referrer: String, rule: String },
    #[error("rule reference cycle: {}", path.join(" -> "))]
    Cycle { path: Vec<String> },
}

/// Serialised form of a policy. `Policy` itself keeps parsed expressions,
/// so parsing happens once at load and evaluation never fails on syntax.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyDocument {
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "expr_map"
    )]
    pub rules: BTreeMap<String, String>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "expr_map"
    )]
    pub actions: BTreeMap<String, String>,
}

/// Accept `"x:y": true` as well as `"x:y": "true"`, since YAML parses a bare
/// `true` as a boolean and that is the obvious thing to write.
fn expr_map<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Scalar {
        Text(String),
        Bool(bool),
    }
    let raw: BTreeMap<String, Scalar> = BTreeMap::deserialize(d)?;
    Ok(raw
        .into_iter()
        .map(|(k, v)| {
            let s = match v {
                Scalar::Text(s) => s,
                Scalar::Bool(b) => b.to_string(),
            };
            (k, s)
        })
        .collect())
}

/// A validated, evaluable policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    rules: BTreeMap<String, Expr>,
    actions: BTreeMap<String, Expr>,
}

impl Default for Policy {
    fn default() -> Self {
        Self::default_policy()
    }
}

impl Policy {
    /// An empty policy: every action is unknown and therefore denied.
    pub fn empty() -> Self {
        Self {
            rules: BTreeMap::new(),
            actions: BTreeMap::new(),
        }
    }

    /// Parse a YAML (or JSON) policy document and validate it: every
    /// expression must parse, every `rule:` reference must resolve, and
    /// rule references must not form a cycle.
    pub fn parse(text: &str) -> Result<Self, PolicyError> {
        let doc: PolicyDocument =
            serde_yaml_ng::from_str(text).map_err(|e| PolicyError::Document(e.to_string()))?;
        Self::from_document(&doc)
    }

    /// Build a policy from an already-deserialised document.
    pub fn from_document(doc: &PolicyDocument) -> Result<Self, PolicyError> {
        let mut rules = BTreeMap::new();
        for (name, src) in &doc.rules {
            let e = expr::parse(src).map_err(|source| PolicyError::RuleSyntax {
                name: name.clone(),
                source,
            })?;
            rules.insert(name.clone(), e);
        }
        let mut actions = BTreeMap::new();
        for (action, src) in &doc.actions {
            let e = expr::parse(src).map_err(|source| PolicyError::ActionSyntax {
                action: action.clone(),
                source,
            })?;
            actions.insert(action.clone(), e);
        }
        let p = Self { rules, actions };
        p.validate()?;
        Ok(p)
    }

    /// The document form, suitable for `serde_yaml_ng::to_string`.
    pub fn to_document(&self) -> PolicyDocument {
        PolicyDocument {
            rules: self
                .rules
                .iter()
                .map(|(k, v)| (k.clone(), v.to_string()))
                .collect(),
            actions: self
                .actions
                .iter()
                .map(|(k, v)| (k.clone(), v.to_string()))
                .collect(),
        }
    }

    /// Named rules, in order.
    pub fn rules(&self) -> impl Iterator<Item = (&str, &Expr)> {
        self.rules.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Known actions, in order.
    pub fn actions(&self) -> impl Iterator<Item = (&str, &Expr)> {
        self.actions.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// The expression guarding `action`, if the policy knows the action.
    pub fn rule_for(&self, action: &str) -> Option<&Expr> {
        self.actions.get(action)
    }

    /// Overlay `overrides`: its rules and actions replace same-named
    /// entries here; everything else is kept. The result is re-validated,
    /// and on failure `self` is left unchanged.
    pub fn merge(&mut self, overrides: Policy) -> Result<(), PolicyError> {
        let mut merged = self.clone();
        merged.rules.extend(overrides.rules);
        merged.actions.extend(overrides.actions);
        merged.validate()?;
        *self = merged;
        Ok(())
    }

    /// Parse `text` and merge it over `self`.
    pub fn merge_text(&mut self, text: &str) -> Result<(), PolicyError> {
        // Parse without cross-validation first (an override may legitimately
        // reference a rule defined only in the base), then validate merged.
        let doc: PolicyDocument =
            serde_yaml_ng::from_str(text).map_err(|e| PolicyError::Document(e.to_string()))?;
        let mut rules = BTreeMap::new();
        for (name, src) in &doc.rules {
            rules.insert(
                name.clone(),
                expr::parse(src).map_err(|source| PolicyError::RuleSyntax {
                    name: name.clone(),
                    source,
                })?,
            );
        }
        let mut actions = BTreeMap::new();
        for (action, src) in &doc.actions {
            actions.insert(
                action.clone(),
                expr::parse(src).map_err(|source| PolicyError::ActionSyntax {
                    action: action.clone(),
                    source,
                })?,
            );
        }
        self.merge(Policy { rules, actions })
    }

    /// `Ok(())` if `caller` may perform `action` on `target`, otherwise a
    /// `forbidden` error naming the action. Unknown actions are denied and
    /// logged at `warn`, since they almost always mean a module added an
    /// action without a default.
    pub fn authorize(
        &self,
        caller: &Caller,
        action: &str,
        target: &Target,
    ) -> Result<(), ApiError> {
        if self.is_allowed(caller, action, target) {
            Ok(())
        } else {
            Err(ApiError::forbidden(format!(
                "{} is not allowed to {action}",
                caller.principal_name
            ))
            .with_detail("action", action)
            .with_detail("principal", caller.principal.to_string()))
        }
    }

    /// The boolean form of [`Policy::authorize`].
    pub fn is_allowed(&self, caller: &Caller, action: &str, target: &Target) -> bool {
        match self.actions.get(action) {
            Some(e) => self.eval(e, caller, target, 0),
            None => {
                tracing::warn!(action, principal = %caller.principal, "policy: unknown action, denying");
                false
            }
        }
    }

    fn eval(&self, e: &Expr, caller: &Caller, target: &Target, depth: usize) -> bool {
        match e {
            Expr::True => true,
            Expr::False => false,
            Expr::System => caller.system,
            Expr::ProjectMember => caller.project.is_some() && caller.project == target.project,
            Expr::Owner => target.owner.as_ref() == Some(&caller.principal),
            Expr::Role(r) => caller.has_role(r),
            Expr::Rule(name) => match self.rules.get(name) {
                // Validation guarantees no cycles; the depth guard is belt
                // and braces so a bug can never spin forever.
                Some(inner) if depth < 64 => self.eval(inner, caller, target, depth + 1),
                Some(_) => {
                    tracing::error!(rule = %name, "policy: rule nesting too deep, denying");
                    false
                }
                None => {
                    tracing::error!(rule = %name, "policy: unknown rule reference, denying");
                    false
                }
            },
            Expr::Not(inner) => !self.eval(inner, caller, target, depth),
            Expr::And(a, b) => {
                self.eval(a, caller, target, depth) && self.eval(b, caller, target, depth)
            }
            Expr::Or(a, b) => {
                self.eval(a, caller, target, depth) || self.eval(b, caller, target, depth)
            }
        }
    }

    fn validate(&self) -> Result<(), PolicyError> {
        for (name, e) in &self.rules {
            for r in e.rule_refs() {
                if !self.rules.contains_key(r) {
                    return Err(PolicyError::UnknownRule {
                        referrer: format!("rule '{name}'"),
                        rule: r.to_owned(),
                    });
                }
            }
        }
        for (action, e) in &self.actions {
            for r in e.rule_refs() {
                if !self.rules.contains_key(r) {
                    return Err(PolicyError::UnknownRule {
                        referrer: format!("action '{action}'"),
                        rule: r.to_owned(),
                    });
                }
            }
        }
        // Cycle detection: DFS over the rule reference graph.
        let mut done: BTreeSet<&str> = BTreeSet::new();
        for name in self.rules.keys() {
            let mut path: Vec<&str> = Vec::new();
            self.visit(name, &mut path, &mut done)?;
        }
        Ok(())
    }

    fn visit<'a>(
        &'a self,
        name: &'a str,
        path: &mut Vec<&'a str>,
        done: &mut BTreeSet<&'a str>,
    ) -> Result<(), PolicyError> {
        if done.contains(name) {
            return Ok(());
        }
        if let Some(start) = path.iter().position(|n| *n == name) {
            let mut cycle: Vec<String> = path[start..].iter().map(|s| (*s).to_owned()).collect();
            cycle.push(name.to_owned());
            return Err(PolicyError::Cycle { path: cycle });
        }
        path.push(name);
        if let Some(e) = self.rules.get(name) {
            for r in e.rule_refs() {
                self.visit(r, path, done)?;
            }
        }
        path.pop();
        done.insert(name);
        Ok(())
    }

    /// The built-in policy. Named rules:
    ///
    /// * `admin`  – `role:admin or system`
    /// * `member` – `rule:admin or (project_member and role:member)`
    /// * `reader` – `rule:member or (project_member and role:reader)`
    /// * `owner_or_admin` – `rule:admin or owner`
    ///
    /// Project-scoped kinds: create/update/delete need `member`, read/list
    /// need `reader`. Cloud-scoped kinds (hosts, flavors, roles, the policy)
    /// are admin-only to write; flavors and public images are readable by
    /// anyone authenticated. Tokens may be created by anyone (that is how
    /// you log in) and read or revoked by their owner.
    pub fn default_policy() -> Self {
        let mut rules = BTreeMap::new();
        let mut actions = BTreeMap::new();
        let mut rule = |name: &str, src: &str| {
            let e = expr::parse(src).unwrap_or_else(|err| panic!("default rule {name}: {err}"));
            rules.insert(name.to_owned(), e);
        };
        rule("admin", "role:admin or system");
        rule("member", "rule:admin or (project_member and role:member)");
        rule("reader", "rule:member or (project_member and role:reader)");
        rule("owner_or_admin", "rule:admin or owner");

        let mut set = |kind: &str, verbs: &[(&str, &str)]| {
            for (verb, src) in verbs {
                let e = expr::parse(src)
                    .unwrap_or_else(|err| panic!("default action {kind}:{verb}: {err}"));
                actions.insert(format!("{kind}:{verb}"), e);
            }
        };

        const PROJECT_SCOPED: &[(&str, &str)] = &[
            ("create", "rule:member"),
            ("read", "rule:reader"),
            ("list", "rule:reader"),
            ("update", "rule:member"),
            ("delete", "rule:member"),
        ];
        const ADMIN_ONLY: &[(&str, &str)] = &[
            ("create", "rule:admin"),
            ("read", "rule:admin"),
            ("list", "rule:admin"),
            ("update", "rule:admin"),
            ("delete", "rule:admin"),
        ];
        // Admin writes, anyone authenticated reads: flavors, images, roles.
        const PUBLIC_READ: &[(&str, &str)] = &[
            ("create", "rule:admin"),
            ("read", "true"),
            ("list", "true"),
            ("update", "rule:admin"),
            ("delete", "rule:admin"),
        ];

        for kind in [
            // forge
            "machine",
            "reservation",
            // loom
            "pod",
            "deployment",
            "replicaset",
            "service",
            "ingress",
            "job",
            "cronjob",
            "configmap",
            // fabric
            "network",
            "subnet",
            "port",
            "router",
            "security_group",
            "floating_ip",
            // slab
            "volume",
            "snapshot",
            "backup",
            // silo
            "bucket",
            "object",
            // shelf
            "share",
            "share_access",
            // beacon
            "zone",
            "record_set",
            // relay
            "balancer",
            "listener",
            "pool",
            "member",
            "health_monitor",
            // keep
            "secret",
            "certificate",
            // blueprint
            "stack",
            "workflow",
            "kit",
            // gauge
            "alarm",
        ] {
            set(kind, PROJECT_SCOPED);
        }
        for kind in [
            "host",
            "policy",
            "role_binding",
            "quota",
            "volume_type",
            "certificate_authority",
            "usage_record",
        ] {
            set(kind, ADMIN_ONLY);
        }
        for kind in ["flavor", "role"] {
            set(kind, PUBLIC_READ);
        }

        // Images: project-owned but readable by everyone (public images).
        set(
            "image",
            &[
                ("create", "rule:member"),
                ("read", "true"),
                ("list", "true"),
                ("update", "rule:member"),
                ("delete", "rule:member"),
            ],
        );
        set("image_member", PROJECT_SCOPED);

        // Projects: only admins make or remove them or list them all;
        // members see and edit their own.
        set(
            "project",
            &[
                ("create", "rule:admin"),
                ("read", "rule:reader"),
                ("list", "rule:admin"),
                ("update", "rule:member"),
                ("delete", "rule:admin"),
            ],
        );
        // Principals (users, service accounts): admin-managed, self-readable.
        set(
            "principal",
            &[
                ("create", "rule:admin"),
                ("read", "rule:owner_or_admin"),
                ("list", "rule:admin"),
                ("update", "rule:owner_or_admin"),
                ("delete", "rule:admin"),
            ],
        );
        // Tokens: anyone may log in; you may inspect or revoke your own.
        set(
            "token",
            &[
                ("create", "true"),
                ("read", "rule:owner_or_admin"),
                ("list", "rule:owner_or_admin"),
                ("update", "false"),
                ("delete", "rule:owner_or_admin"),
            ],
        );
        // Metrics: readable in-project, written only by gauge itself.
        set(
            "metric",
            &[
                ("create", "rule:admin"),
                ("read", "rule:reader"),
                ("list", "rule:reader"),
                ("update", "rule:admin"),
                ("delete", "rule:admin"),
            ],
        );
        // Machine lifecycle verbs beyond CRUD.
        set(
            "machine",
            &[
                ("start", "rule:member"),
                ("stop", "rule:member"),
                ("reboot", "rule:member"),
                ("console", "rule:member"),
                ("migrate", "rule:admin"),
                ("evacuate", "rule:admin"),
            ],
        );
        set(
            "volume",
            &[("attach", "rule:member"), ("detach", "rule:member")],
        );
        set(
            "floating_ip",
            &[
                ("associate", "rule:member"),
                ("disassociate", "rule:member"),
            ],
        );
        set(
            "host",
            &[
                ("drain", "rule:admin"),
                ("enable", "rule:admin"),
                ("disable", "rule:admin"),
            ],
        );

        let p = Self { rules, actions };
        // A broken default is a programming error, not a runtime condition.
        if let Err(e) = p.validate() {
            panic!("default policy is invalid: {e}");
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let e =
            Policy::parse("rules:\n  a: \"rule:b\"\n  b: \"rule:c\"\n  c: \"rule:a or owner\"\n")
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
}
