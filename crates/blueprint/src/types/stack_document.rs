// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `StackDocument`: the shared behaviour of a stack and a kit spec.
use pistack_types::{ApiError, ApiResult};
use std::collections::BTreeMap;

use super::parameter_decl::ParameterDecl;
use super::template::{declared_uses, given, names, push, substitute, PARAM_PREFIX};

/// A stack spec: the document, the parameters it declares, and the values
/// given for them.
pub trait StackDocument {
    /// The user's stack text, YAML or JSON. Stored as given.
    fn document(&self) -> &str;
    /// The parameters the document may name.
    fn declared(&self) -> &[ParameterDecl];
    /// Values given for the declared parameters.
    fn values(&self) -> &BTreeMap<String, String>;
    fn declared_mut(&mut self) -> &mut Vec<ParameterDecl>;

    /// Trim every declared name, refuse a name declared twice, refuse a value
    /// no parameter was declared for, and refuse a declared parameter the
    /// document never names.
    fn normalize(&mut self) -> ApiResult<()> {
        for decl in self.declared_mut() {
            decl.normalized()?;
        }
        let decls = self.declared();
        if let Some(dup) = (1..decls.len()).find(|i| decls[*i].name == decls[i - 1].name) {
            return Err(ApiError::bad_request(format!(
                "spec.declared declares '{}' twice",
                decls[dup].name
            )));
        }
        let named = declared_uses(self.document());
        let mut unknown = Vec::new();
        for given in self.values().keys() {
            let asked = given.trim().trim_start_matches(PARAM_PREFIX);
            if !self.declared().iter().any(|decl| names(&decl.name, asked)) {
                push(&mut unknown, asked.to_owned());
            }
        }
        if !unknown.is_empty() {
            return Err(ApiError::bad_request(format!(
                "no parameter is declared for '{}': declare it or drop the value",
                unknown.join("', '")
            )));
        }
        let mut unused = Vec::new();
        for decl in self.declared() {
            if !named.iter().any(|name| names(&decl.name, name)) {
                push(&mut unused, decl.name.clone());
            }
        }
        if !unused.is_empty() {
            return Err(ApiError::bad_request(format!(
                "the document never names declared parameter '{}'",
                unused.join("', '")
            )));
        }
        Ok(())
    }

    /// The value a reference to `param` resolves to: the one given, else the
    /// declared default, else nothing.
    fn value_of(&self, param: &str) -> Option<String> {
        given(self.values(), param).or_else(|| {
            self.declared()
                .iter()
                .find(|decl| names(&decl.name, param))
                .and_then(ParameterDecl::resolved_default)
        })
    }

    /// Refuse a reference to a parameter nothing declares, and a reference to
    /// a declared parameter with neither a value nor a default. Each message
    /// names what is wrong.
    fn check_refs(&self) -> ApiResult<()> {
        let mut missing = Vec::new();
        let mut undeclared = Vec::new();
        for name in declared_uses(self.document()) {
            if !self.declared().iter().any(|decl| names(&decl.name, &name)) {
                push(&mut undeclared, name);
            } else if self.value_of(&name).is_none() {
                push(&mut missing, name);
            }
        }
        if !missing.is_empty() {
            return Err(ApiError::bad_request(format!(
                "no value given for parameter '{}' and it has no default",
                missing.join("', '")
            )));
        }
        if !undeclared.is_empty() {
            return Err(ApiError::bad_request(format!(
                "the document names undeclared parameter '{}'",
                undeclared.join("', '")
            )));
        }
        Ok(())
    }

    /// The document with every reference replaced by the value it resolves
    /// to. A name nothing declares and a value that is required and missing
    /// are refused here, so nothing half-substituted reaches a row. A
    /// document with no reference is returned as it is stored.
    fn realized(&self) -> ApiResult<String> {
        self.check_refs()?;
        Ok(substitute(self.document(), self.declared(), self.values())?
            .unwrap_or_else(|| self.document().to_owned()))
    }
}
