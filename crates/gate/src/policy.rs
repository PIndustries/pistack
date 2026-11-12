// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Built-in RBAC. Deliberately small; `pistack-policy` replaces this with a
//! rule language later, behind the same `authorize` signature.
//!
//! Actions are `"<resource>.<verb>"`, e.g. `machines.create`, `projects.list`.
//! Verbs `get`, `list`, `read`, `watch` are reads; everything else is a write.
//!
//! * `admin` bound at system scope may do anything.
//! * `admin` bound in a project, and `member`, may read and write within the
//!   project the token is scoped to.
//! * `reader` may read within the project the token is scoped to.
//! * Anything with no project (`project: None`) is a system action: admin only.

use pistack_types::{ApiError, ApiResult, Caller, Id};
use serde::Serialize;

use crate::types::{ROLE_ADMIN, ROLE_MEMBER, ROLE_READER};

const READ_VERBS: [&str; 4] = ["get", "list", "read", "watch"];

pub fn is_read(action: &str) -> bool {
    let verb = action.rsplit('.').next().unwrap_or(action);
    READ_VERBS.contains(&verb)
}

/// May `caller` perform `action`, optionally within `project`?
pub fn authorize(caller: &Caller, action: &str, project: Option<&Id>) -> ApiResult<()> {
    if caller.is_admin() {
        return Ok(());
    }
    let deny = |why: &str| {
        Err(ApiError::forbidden(format!("{action}: {why}"))
            .with_detail("action", action)
            .with_detail("principal", caller.principal_name.as_str()))
    };
    let Some(project) = project else {
        return deny("requires the admin role");
    };
    if caller.project.as_ref() != Some(project) {
        return deny("token is not scoped to this project");
    }
    if caller.has_role(ROLE_MEMBER) || caller.has_role(ROLE_ADMIN) {
        return Ok(());
    }
    if caller.has_role(ROLE_READER) && is_read(action) {
        return Ok(());
    }
    if caller.has_role(ROLE_READER) {
        return deny("reader role is read-only");
    }
    deny("no role in this project")
}

/// The policy as `GET /identity/policy` describes it.
#[derive(Clone, Debug, Serialize)]
pub struct PolicyDocument {
    pub engine: &'static str,
    pub roles: Vec<PolicyRole>,
    pub read_verbs: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PolicyRole {
    pub name: &'static str,
    pub scope: &'static str,
    pub allows: &'static str,
}

pub fn document() -> PolicyDocument {
    PolicyDocument {
        engine: "builtin-rbac",
        roles: vec![
            PolicyRole {
                name: ROLE_ADMIN,
                scope: "system",
                allows: "*",
            },
            PolicyRole {
                name: ROLE_ADMIN,
                scope: "project",
                allows: "read and write within the scoped project",
            },
            PolicyRole {
                name: ROLE_MEMBER,
                scope: "project",
                allows: "read and write within the scoped project",
            },
            PolicyRole {
                name: ROLE_READER,
                scope: "project",
                allows: "read within the scoped project",
            },
        ],
        read_verbs: READ_VERBS.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_types::kind;

    fn caller(roles: &[&str], project: Option<&Id>, system: bool) -> Caller {
        Caller {
            principal: Id::new(kind::PRINCIPAL),
            principal_name: "t".into(),
            project: project.cloned(),
            roles: roles.iter().map(|s| s.to_string()).collect(),
            system,
        }
    }

    #[test]
    fn matrix() {
        let p = Id::new(kind::PROJECT);
        let other = Id::new(kind::PROJECT);
        let sys_admin = caller(&["admin"], None, true);
        let proj_admin = caller(&["admin"], Some(&p), false);
        let member = caller(&["member"], Some(&p), false);
        let reader = caller(&["reader"], Some(&p), false);
        let nobody = caller(&[], Some(&p), false);

        // (caller, action, project, allowed)
        let cases: Vec<(&Caller, &str, Option<&Id>, bool)> = vec![
            (&sys_admin, "projects.list", None, true),
            (&sys_admin, "machines.create", Some(&other), true),
            (&proj_admin, "projects.list", None, false),
            (&proj_admin, "machines.create", Some(&p), true),
            (&proj_admin, "machines.create", Some(&other), false),
            (&member, "projects.list", None, false),
            (&member, "machines.create", Some(&p), true),
            (&member, "machines.get", Some(&p), true),
            (&member, "machines.create", Some(&other), false),
            (&reader, "machines.get", Some(&p), true),
            (&reader, "machines.list", Some(&p), true),
            (&reader, "machines.create", Some(&p), false),
            (&reader, "machines.delete", Some(&p), false),
            (&reader, "machines.get", Some(&other), false),
            (&nobody, "machines.get", Some(&p), false),
            (&nobody, "projects.list", None, false),
        ];
        for (c, action, project, allowed) in cases {
            let r = authorize(c, action, project);
            assert_eq!(
                r.is_ok(),
                allowed,
                "{:?} {action} {:?} -> {r:?}",
                c.roles,
                project
            );
            if let Err(e) = r {
                assert_eq!(e.code, pistack_types::error::ErrorCode::Forbidden);
            }
        }
    }

    #[test]
    fn read_verbs() {
        assert!(is_read("machines.get"));
        assert!(is_read("machines.list"));
        assert!(!is_read("machines.create"));
        assert!(!is_read("machines"));
    }
}
