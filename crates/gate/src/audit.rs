// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Append-only record of mutating API calls.
//!
//! Each record is a store resource: method, path, principal name, status.
//! Request bodies, query strings and headers are not stored. `GET`, `HEAD`
//! and `OPTIONS` are skipped, as are paths under `/healthz`, `/rpc` and
//! `/metrics`.

use pistack_store::Store;
use pistack_types::{Meta, Resource, Spec};
use serde::{Deserialize, Serialize};

const KIND: &str = "audit";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditSpec {
    pub method: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    pub status: u16,
}

impl Spec for AuditSpec {
    const KIND: &'static str = KIND;
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditStatus {}

type AuditEvent = Resource<AuditSpec, AuditStatus>;

/// Persist one audit record. Safe methods and health, rpc and metrics paths
/// are ignored. Store errors are not returned to the caller.
pub fn record(store: &Store, method: &str, path: &str, principal: Option<&str>, status: u16) {
    if skip(method, path) {
        return;
    }
    let path = strip_query(path);
    let mut meta = Meta::new(KIND, "audit", None);
    meta.name = meta.id.to_string();
    let event = AuditEvent {
        meta,
        spec: AuditSpec {
            method: method.to_owned(),
            path: path.to_owned(),
            principal: principal.map(str::to_owned),
            status,
        },
        status: AuditStatus {},
    };
    if let Err(e) = store.collection::<AuditEvent>().create(event) {
        tracing::debug!(error = %e, "audit record was not stored");
    }
}

fn skip(method: &str, path: &str) -> bool {
    if method.eq_ignore_ascii_case("GET")
        || method.eq_ignore_ascii_case("HEAD")
        || method.eq_ignore_ascii_case("OPTIONS")
    {
        return true;
    }
    let path = strip_query(path);
    path.starts_with("/healthz") || path.starts_with("/rpc") || path.starts_with("/metrics")
}

fn strip_query(path: &str) -> &str {
    path.split(['?', '#']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_is_stored_and_get_is_not() {
        let store = Store::in_memory().unwrap();
        record(
            &store,
            "POST",
            "/identity/projects?token=secret",
            Some("alice"),
            201,
        );
        record(&store, "GET", "/identity/projects", Some("alice"), 200);
        record(&store, "HEAD", "/identity/projects", None, 200);
        record(&store, "OPTIONS", "/identity/projects", None, 204);
        record(&store, "POST", "/healthz", Some("alice"), 200);
        record(&store, "POST", "/rpc/agents", Some("alice"), 200);
        record(&store, "POST", "/metrics", Some("alice"), 200);
        let items = store.collection::<AuditEvent>().list(None).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].spec.method, "POST");
        assert_eq!(items[0].spec.path, "/identity/projects");
        assert!(!items[0].spec.path.contains('?'));
        assert_eq!(items[0].spec.principal.as_deref(), Some("alice"));
        assert_eq!(items[0].spec.status, 201);
    }
}
