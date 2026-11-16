// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The body `pi binding create` sends. The scope must be named on the
//! command line: the global `--project` flag is easy to mistake for one,
//! and a binding with no project is cloud-wide.

use super::args::BindingCreate;
use super::client::{CliError, CliResult};
use serde_json::{json, Value};

pub(crate) fn create_body(a: &BindingCreate) -> CliResult<Value> {
    if a.in_project.is_none() && !a.system {
        return Err(CliError::usage(
            "say where the binding applies: --in-project PROJECT for one project, or --system for the whole cloud",
        ));
    }
    Ok(json!({
        "principal": a.principal,
        "role": a.role,
        "project": a.in_project,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(in_project: Option<&str>, system: bool) -> BindingCreate {
        BindingCreate {
            principal: "alice".into(),
            role: "member".into(),
            in_project: in_project.map(str::to_owned),
            system,
        }
    }

    #[test]
    fn a_binding_that_names_no_scope_is_refused() {
        let e = create_body(&args(None, false)).unwrap_err();
        assert_eq!(
            e.to_string(),
            "say where the binding applies: --in-project PROJECT for one project, or --system for the whole cloud"
        );
    }

    #[test]
    fn system_scope_sends_no_project() {
        assert_eq!(
            create_body(&args(None, true)).unwrap(),
            json!({
                "principal": "alice", "role": "member", "project": null,
            })
        );
    }

    #[test]
    fn a_named_project_goes_into_the_body() {
        assert_eq!(
            create_body(&args(Some("web"), false)).unwrap(),
            json!({
                "principal": "alice", "role": "member", "project": "web",
            })
        );
    }
}
