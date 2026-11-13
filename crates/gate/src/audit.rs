// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The audit log: who did what, from where, and how it ended.
//!
//! [`layer`] is gateway middleware. After the response it stores one record
//! for every request that changes something, and for the few reads that hand
//! out a secret ([`audited_read`]). A record holds the method and path, what
//! they mean (action, resource kind and id), the caller (principal, the API
//! key behind the token, project), the client address, the status and the
//! request id. Request bodies, query strings and headers are never stored,
//! so no password, token or secret value reaches a record.
//!
//! Records are store resources, so they replicate with everything else.
//! [`sweep`] deletes the ones past [`Retention`]; gate runs it on the leader.
//! [`list`] is the query behind `GET /identity/audit`.

use axum::{
    body::{Body, HttpBody},
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use chrono::{DateTime, Duration, Utc};
use pistack_store::Store;
use pistack_types::{Caller, Id, Meta, Page, Resource, Spec};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

const KIND: &str = "audit";

/// Days a record is kept. `0` keeps records until the count limit drops them.
pub const MAX_AGE_ENV: &str = "PISTACK_AUDIT_MAX_AGE_DAYS";
/// Most records kept. The oldest go first.
pub const MAX_RECORDS_ENV: &str = "PISTACK_AUDIT_MAX_RECORDS";
pub const DEFAULT_MAX_AGE_DAYS: u32 = 90;
pub const DEFAULT_MAX_RECORDS: usize = 50_000;
/// How often the leader sweeps.
pub const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(300);
/// Most records one sweep deletes; the next one continues.
const SWEEP_BATCH: usize = 5_000;
/// A created resource's id is read from a reply no larger than this.
const CREATED_BODY_MAX: usize = 256 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditSpec {
    pub method: String,
    pub path: String,
    /// What the request does: `create`, `update`, `delete`, `login`, the
    /// verb of an action route (`drain`, `evacuate`, `start`), or a verb and
    /// the part of the resource it touches (`update_quotas`, `read_payload`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub action: String,
    /// The collection in the path, such as `machines` or `bindings`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The resource as the path names it (an id or a name), or the id of
    /// the resource a create made.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resource: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<Id>,
    /// The API key the caller's token was issued for, when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<Id>,
    /// The project of the caller's token, or of the resource a create made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<Id>,
    /// The peer address of the connection, as the gateway saw it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl Spec for AuditSpec {
    const KIND: &'static str = KIND;
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditStatus {}

pub type AuditEvent = Resource<AuditSpec, AuditStatus>;

/// Request extension: the address the client connected from. The gateway
/// sets it from the socket; a proxy in front shows up as the proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientAddr(pub IpAddr);

/// Request extension set by `attach_caller`: the API key in the token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenKey(pub Id);

/// Response extension a handler sets to say who a request was about when
/// there is no caller to read it from: a login, failed or not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Subject {
    pub principal: Option<String>,
    pub principal_id: Option<Id>,
    pub api_key: Option<Id>,
    pub project: Option<Id>,
}

/// A read that is recorded like a write, because it returns a secret or
/// control of a resource: a secret's payload, a certificate's private key,
/// a machine console ticket.
pub fn audited_read(path: &str) -> bool {
    let s: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    matches!(
        s.as_slice(),
        ["secrets", "secrets", _, "payload"]
            | ["secrets", "certificates", _, "key"]
            | ["compute", "machines", _, "console"]
    )
}

fn is_read(method: &str) -> bool {
    method.eq_ignore_ascii_case("GET")
        || method.eq_ignore_ascii_case("HEAD")
        || method.eq_ignore_ascii_case("OPTIONS")
}

/// Whether a request is recorded. Reads are not, but for [`audited_read`].
/// The agent transport is not, but for enrolment, which spends a join token
/// and issues a certificate. Object data is not: one record per object
/// written would push everything else out of the log.
pub fn recorded(method: &str, path: &str) -> bool {
    let path = strip_query(path);
    if is_read(method) {
        return method.eq_ignore_ascii_case("GET") && audited_read(path);
    }
    let s: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    match s.as_slice() {
        ["healthz", ..] | ["metrics", ..] => false,
        ["rpc", rest @ ..] => rest == ["agents", "enroll"],
        ["silo", "buckets"] | ["silo", "buckets", _] => true,
        ["silo", ..] => false,
        _ => true,
    }
}

/// Action, kind and resource for a request, read from its method and path.
pub fn describe(method: &str, path: &str) -> (String, String, String) {
    let s: Vec<&str> = strip_query(path)
        .split('/')
        .filter(|p| !p.is_empty())
        .collect();
    let verb = match method.to_ascii_uppercase().as_str() {
        "POST" => "create",
        "PUT" | "PATCH" => "update",
        "DELETE" => "delete",
        _ => "read",
    };
    match s.as_slice() {
        ["identity", "tokens"] => ("login".into(), "tokens".into(), String::new()),
        ["rpc", "agents", "enroll"] => ("enroll".into(), "agents".into(), String::new()),
        [_, kind] => (verb.into(), (*kind).into(), String::new()),
        [_, kind, id] => (verb.into(), (*kind).into(), (*id).into()),
        [_, kind, id, sub, ..] => {
            let action = if verb == "create" {
                (*sub).to_owned()
            } else {
                format!("{verb}_{sub}")
            };
            (action.replace('-', "_"), (*kind).into(), (*id).into())
        }
        [one] => (verb.into(), (*one).into(), String::new()),
        [] => (verb.into(), String::new(), String::new()),
    }
}

fn strip_query(path: &str) -> &str {
    path.split(['?', '#']).next().unwrap_or(path)
}

/// Everything known about a request once it has been answered.
#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub method: String,
    pub path: String,
    pub caller: Option<Caller>,
    pub subject: Option<Subject>,
    pub api_key: Option<Id>,
    pub client: Option<IpAddr>,
    pub status: u16,
    pub request_id: Option<String>,
    /// Id and project of the resource a create returned.
    pub created: Option<(String, Option<Id>)>,
}

/// Store one record, if the request is one that is recorded. A store error
/// is logged and not returned: on a follower the store refuses the write.
pub fn record(store: &Store, e: Entry) {
    if !recorded(&e.method, &e.path) {
        return;
    }
    let path = strip_query(&e.path).to_owned();
    let (action, kind, mut resource) = describe(&e.method, &path);
    let subject = e.subject.unwrap_or_default();
    let mut project = e
        .caller
        .as_ref()
        .and_then(|c| c.project.clone())
        .or(subject.project);
    if let Some((id, created_project)) = e.created {
        if resource.is_empty() {
            resource = id;
        }
        if created_project.is_some() {
            project = created_project;
        }
    }
    let mut meta = Meta::new(KIND, "audit", None);
    meta.name = meta.id.to_string();
    let event = AuditEvent {
        meta,
        spec: AuditSpec {
            method: e.method.to_ascii_uppercase(),
            path,
            action,
            kind,
            resource,
            principal: e
                .caller
                .as_ref()
                .map(|c| c.principal_name.clone())
                .or(subject.principal),
            principal_id: e
                .caller
                .as_ref()
                .map(|c| c.principal.clone())
                .or(subject.principal_id),
            api_key: e.api_key.or(subject.api_key),
            project,
            client: e.client.map(|ip| ip.to_string()),
            status: e.status,
            request_id: e.request_id,
        },
        status: AuditStatus {},
    };
    if let Err(err) = store.collection::<AuditEvent>().create(event) {
        tracing::debug!(error = %err, "audit record was not stored");
    }
}

/// Gateway middleware: `from_fn_with_state(store, gate::audit::layer)`.
/// Inner to authentication, so the caller is known, and outer to the role
/// check, so a refused request is recorded with its 403.
pub async fn layer(State(store): State<Store>, req: Request, next: Next) -> Response {
    let method = req.method().as_str().to_owned();
    let path = req.uri().path().to_owned();
    if !recorded(&method, &path) {
        return next.run(req).await;
    }
    let ext = req.extensions();
    let caller = ext.get::<Caller>().cloned();
    let api_key = ext.get::<TokenKey>().map(|k| k.0.clone());
    let client = ext.get::<ClientAddr>().map(|c| c.0);
    let request_id = request_id(req.headers());
    let response = next.run(req).await;
    let status = response.status();
    let subject = response.extensions().get::<Subject>().cloned();
    let (response, created) = if status == StatusCode::CREATED {
        created_resource(response).await
    } else {
        (response, None)
    };
    record(
        &store,
        Entry {
            method,
            path,
            caller,
            subject,
            api_key,
            client,
            status: status.as_u16(),
            request_id,
            created,
        },
    );
    response
}

fn request_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 128)
        .map(str::to_owned)
}

#[derive(Deserialize)]
struct Created {
    id: String,
    #[serde(default)]
    project: Option<Id>,
}

/// The `id` and `project` of a `201` JSON reply. Only those two fields are
/// read; a reply that also carries a new key's secret gives up nothing else.
/// A reply of unknown or large size is passed through unread.
async fn created_resource(response: Response) -> (Response, Option<(String, Option<Id>)>) {
    let json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    let small = HttpBody::size_hint(response.body())
        .upper()
        .is_some_and(|n| n <= CREATED_BODY_MAX as u64);
    if !json || !small {
        return (response, None);
    }
    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, CREATED_BODY_MAX).await else {
        // Unreachable for a body no longer than the size it gave.
        return (Response::from_parts(parts, Body::empty()), None);
    };
    let created = serde_json::from_slice::<Created>(&bytes)
        .ok()
        .filter(|c| !c.id.is_empty() && c.id.len() <= 128)
        .map(|c| (c.id, c.project));
    (Response::from_parts(parts, Body::from(bytes)), created)
}

// ---- reading ---------------------------------------------------------------

/// Filters of `GET /identity/audit`. Every one that is set must match.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AuditQuery {
    /// Records made at or after this time (RFC 3339).
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    /// Records made before this time.
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    /// Principal name or id.
    #[serde(default)]
    pub principal: Option<String>,
    /// Project id. The handler turns a name into the id first.
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    /// Resource as recorded: the id or name the request used.
    #[serde(default)]
    pub resource: Option<String>,
    /// `success` (below 400), `failure` (400 and up) or one status code.
    #[serde(default)]
    pub outcome: Option<String>,
    /// `asc`, oldest first (the default), or `desc`.
    #[serde(default)]
    pub order: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    /// The `next` of the page before.
    #[serde(default)]
    pub after: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Success,
    Failure,
    Status(u16),
}

fn parse_outcome(raw: &str) -> Result<Outcome, String> {
    match raw.trim() {
        "success" | "ok" => Ok(Outcome::Success),
        "failure" | "failed" | "refused" => Ok(Outcome::Failure),
        n => n
            .parse::<u16>()
            .ok()
            .filter(|s| (100..600).contains(s))
            .map(Outcome::Status)
            .ok_or_else(|| "outcome must be success, failure or a status code".to_owned()),
    }
}

/// One page of records matching `q`. `total` counts every match, before
/// `after` and `limit`. Reads the whole log: with the default retention
/// that is at most 50,000 small records.
pub fn list(store: &Store, q: &AuditQuery) -> Result<Page<AuditEvent>, String> {
    let outcome = q.outcome.as_deref().map(parse_outcome).transpose()?;
    let desc = match q.order.as_deref().map(str::trim) {
        None | Some("") | Some("asc") => false,
        Some("desc") => true,
        Some(_) => return Err("order must be asc or desc".into()),
    };
    let wanted = |f: &Option<String>, v: &str| f.as_deref().is_none_or(|w| w == v);
    let mut items = store
        .collection::<AuditEvent>()
        .list(None)
        .map_err(|e| e.to_string())?;
    items.retain(|r| {
        let s = &r.spec;
        q.since.is_none_or(|t| r.meta.created_at >= t)
            && q.until.is_none_or(|t| r.meta.created_at < t)
            && q.principal.as_deref().is_none_or(|p| {
                s.principal.as_deref() == Some(p)
                    || s.principal_id
                        .as_ref()
                        .is_some_and(|id| id.to_string() == p)
            })
            && q.project
                .as_deref()
                .is_none_or(|p| s.project.as_ref().is_some_and(|id| id.to_string() == p))
            && wanted(&q.action, &s.action)
            && wanted(&q.kind, &s.kind)
            && wanted(&q.resource, &s.resource)
            && outcome.is_none_or(|o| match o {
                Outcome::Success => s.status < 400,
                Outcome::Failure => s.status >= 400,
                Outcome::Status(code) => s.status == code,
            })
    });
    sort_oldest_first(&mut items);
    if desc {
        items.reverse();
    }
    let total = items.len();
    if let Some(after) = q.after.as_deref() {
        match items.iter().position(|r| r.meta.id.to_string() == after) {
            Some(at) => {
                items.drain(..=at);
            }
            // The record the cursor names was swept. Ids are UUIDv7, so
            // they order by time to the millisecond.
            None => items.retain(|r| {
                let id = r.meta.id.to_string();
                if desc {
                    id.as_str() < after
                } else {
                    id.as_str() > after
                }
            }),
        }
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let next = (items.len() > limit).then(|| items[limit - 1].meta.id.to_string());
    items.truncate(limit);
    Ok(Page { items, next, total })
}

fn sort_oldest_first(items: &mut [AuditEvent]) {
    items.sort_by(|a, b| {
        (a.meta.created_at, a.meta.id.to_string()).cmp(&(b.meta.created_at, b.meta.id.to_string()))
    });
}

// ---- retention -------------------------------------------------------------

/// How much of the log is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    /// Records older than this are deleted. `None` keeps them by age.
    pub max_age: Option<Duration>,
    /// More records than this and the oldest are deleted.
    pub max_records: usize,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            max_age: Some(Duration::days(i64::from(DEFAULT_MAX_AGE_DAYS))),
            max_records: DEFAULT_MAX_RECORDS,
        }
    }
}

impl Retention {
    /// From [`MAX_AGE_ENV`] and [`MAX_RECORDS_ENV`].
    pub fn from_env() -> Self {
        Self::from_values(
            std::env::var(MAX_AGE_ENV).ok().as_deref(),
            std::env::var(MAX_RECORDS_ENV).ok().as_deref(),
        )
    }

    /// A value that is not a number keeps the default. Zero days turns the
    /// age limit off. The count limit cannot be turned off: zero keeps the
    /// default, so the log always has a bound.
    pub fn from_values(age_days: Option<&str>, records: Option<&str>) -> Self {
        let mut r = Self::default();
        match age_days.map(|v| v.trim().parse::<u32>()) {
            Some(Ok(0)) => r.max_age = None,
            Some(Ok(days)) => r.max_age = Some(Duration::days(i64::from(days))),
            Some(Err(_)) => {
                tracing::warn!("{MAX_AGE_ENV} is not a number of days; using the default")
            }
            None => {}
        }
        match records.map(|v| v.trim().parse::<usize>()) {
            Some(Ok(n)) if n > 0 => r.max_records = n,
            Some(_) => {
                tracing::warn!("{MAX_RECORDS_ENV} is not a positive number; using the default")
            }
            None => {}
        }
        r
    }
}

/// Delete records past `keep`, oldest first, at most [`SWEEP_BATCH`] a call.
/// Returns how many were deleted.
pub fn sweep(store: &Store, keep: &Retention, now: DateTime<Utc>) -> pistack_store::Result<usize> {
    let col = store.collection::<AuditEvent>();
    let mut items = col.list(None)?;
    sort_oldest_first(&mut items);
    let over = items.len().saturating_sub(keep.max_records);
    let cutoff = keep.max_age.map(|age| now - age);
    let mut deleted = 0;
    for (i, r) in items.iter().enumerate() {
        let old = cutoff.is_some_and(|c| r.meta.created_at < c);
        if (i >= over && !old) || deleted >= SWEEP_BATCH {
            break;
        }
        match col.delete(&r.meta.id) {
            Ok(_) | Err(pistack_store::StoreError::NotFound(_)) => deleted += 1,
            Err(e) => return Err(e),
        }
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(method: &str, path: &str, status: u16) -> Entry {
        Entry {
            method: method.into(),
            path: path.into(),
            status,
            ..Entry::default()
        }
    }

    fn all(store: &Store) -> Vec<AuditEvent> {
        list(store, &AuditQuery::default()).unwrap().items
    }

    #[test]
    fn writes_are_stored_and_plain_reads_are_not() {
        let store = Store::in_memory().unwrap();
        record(
            &store,
            entry("POST", "/identity/projects?token=secret", 201),
        );
        for (method, path) in [
            ("GET", "/identity/projects"),
            ("HEAD", "/identity/projects"),
            ("OPTIONS", "/identity/projects"),
            ("POST", "/healthz"),
            ("POST", "/rpc/agents/heartbeat"),
            ("POST", "/metrics"),
            ("PUT", "/silo/photos/cat.jpg"),
            ("PUT", "/silo/buckets/photos/objects/cat.jpg"),
        ] {
            record(&store, entry(method, path, 200));
        }
        let items = all(&store);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].spec.method, "POST");
        assert_eq!(items[0].spec.path, "/identity/projects");
        assert_eq!(items[0].spec.status, 201);
    }

    #[test]
    fn reads_that_return_a_secret_are_stored() {
        let store = Store::in_memory().unwrap();
        for path in [
            "/secrets/secrets/db/payload",
            "/secrets/certificates/web/key",
            "/compute/machines/mach-1/console",
        ] {
            assert!(audited_read(path), "{path}");
            record(&store, entry("GET", path, 200));
        }
        assert!(!audited_read("/secrets/secrets/db"));
        assert!(!audited_read("/compute/machines/mach-1"));
        let actions: Vec<String> = all(&store).into_iter().map(|r| r.spec.action).collect();
        assert_eq!(actions, ["read_payload", "read_key", "read_console"]);
    }

    #[test]
    fn a_path_is_read_into_action_kind_and_resource() {
        let d = |m: &str, p: &str| {
            let (a, k, r) = describe(m, p);
            format!("{a} {k} {r}")
        };
        assert_eq!(d("POST", "/identity/tokens"), "login tokens ");
        assert_eq!(d("POST", "/compute/machines"), "create machines ");
        assert_eq!(d("DELETE", "/compute/machines/web"), "delete machines web");
        assert_eq!(d("POST", "/compute/hosts/rack1/drain"), "drain hosts rack1");
        assert_eq!(
            d("PUT", "/identity/projects/web/quotas"),
            "update_quotas projects web"
        );
        assert_eq!(d("DELETE", "/cluster/members/n2"), "delete members n2");
        assert_eq!(
            d("POST", "/secrets/join-tokens/redeem"),
            "create join-tokens redeem"
        );
        assert_eq!(d("POST", "/rpc/agents/enroll"), "enroll agents ");
    }

    #[test]
    fn retention_reads_its_two_settings() {
        assert_eq!(Retention::from_values(None, None), Retention::default());
        let r = Retention::from_values(Some("7"), Some("200"));
        assert_eq!(r.max_age, Some(Duration::days(7)));
        assert_eq!(r.max_records, 200);
        // No age limit is allowed; no count limit is not.
        let r = Retention::from_values(Some("0"), Some("0"));
        assert_eq!(r.max_age, None);
        assert_eq!(r.max_records, DEFAULT_MAX_RECORDS);
        assert_eq!(
            Retention::from_values(Some("soon"), Some("many")),
            Retention::default()
        );
    }

    #[test]
    fn a_sweep_drops_the_oldest_past_the_count_and_everything_past_the_age() {
        let store = Store::in_memory().unwrap();
        let col = store.collection::<AuditEvent>();
        let now = Utc::now();
        for i in 0..10 {
            record(
                &store,
                entry("POST", &format!("/compute/machines/m{i}/start"), 200),
            );
        }
        // The two oldest were made 40 days ago.
        let mut items = all(&store);
        for r in items.iter_mut().take(2) {
            col.modify(&r.meta.id, |x| {
                x.meta.created_at = now - Duration::days(40);
                Ok(())
            })
            .unwrap();
        }
        let keep = Retention {
            max_age: Some(Duration::days(30)),
            max_records: 100,
        };
        assert_eq!(sweep(&store, &keep, now).unwrap(), 2);
        assert_eq!(all(&store).len(), 8);

        let keep = Retention {
            max_age: None,
            max_records: 5,
        };
        assert_eq!(sweep(&store, &keep, now).unwrap(), 3);
        let left: Vec<String> = all(&store).into_iter().map(|r| r.spec.resource).collect();
        assert_eq!(left, ["m5", "m6", "m7", "m8", "m9"]);
        assert_eq!(sweep(&store, &keep, now).unwrap(), 0);
    }
}
