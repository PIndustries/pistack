// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Policy`: a validated, evaluable policy and its evaluator.

use std::collections::{BTreeMap, BTreeSet};

use pistack_types::{ApiError, Caller};

use super::super::expr::{self, Expr};
use super::policy_document::PolicyDocument;
use super::policy_error::PolicyError;
use super::target::Target;

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
            "statefulset",
            "daemonset",
            "replicaset",
            "service",
            "ingress",
            "job",
            "cronjob",
            "configmap",
            // fabric
            "vpc",
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
