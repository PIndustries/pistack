// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A thin HTTP client over the PiStack API: bearer token, JSON in and out,
//! and the one error envelope every module returns.

use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Wire form of an API error: `{"error":{"code","message","details"}}`.
#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: serde_json::Map<String, Value>,
}

#[derive(Debug, serde::Deserialize)]
struct Envelope {
    error: ErrorBody,
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// The server answered with an error status.
    #[error("{message}")]
    Api {
        status: u16,
        code: String,
        message: String,
        details: serde_json::Map<String, Value>,
    },
    /// Could not reach the server, or it spoke something other than JSON.
    #[error("{0}")]
    Transport(String),
    /// Bad arguments, missing config, user said no.
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl CliError {
    pub fn usage(m: impl Into<String>) -> Self {
        CliError::Usage(m.into())
    }
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, CliError::Api { status: 401, .. })
    }
    pub fn is_not_found(&self) -> bool {
        matches!(self, CliError::Api { status: 404, .. })
    }
    /// True when the server refused a create because that resource name is
    /// already filed in this project. `name` is what was asked for, since one
    /// status covers several refusals and only the one naming it is this.
    pub fn is_name_taken(&self, name: &str) -> bool {
        matches!(self, CliError::Api { status: 409, message, .. } if message.contains(name))
    }
}

impl From<reqwest::Error> for CliError {
    fn from(e: reqwest::Error) -> Self {
        CliError::Transport(e.to_string())
    }
}
impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Other(e.into())
    }
}
impl From<serde_json::Error> for CliError {
    fn from(e: serde_json::Error) -> Self {
        CliError::Transport(format!("invalid json from server: {e}"))
    }
}

pub type CliResult<T> = Result<T, CliError>;

/// Parse an error response body. Falls back to the raw text when it is not
/// the standard envelope (axum's own rejections, proxies, ...).
pub fn parse_error(status: StatusCode, body: &str) -> CliError {
    if let Ok(env) = serde_json::from_str::<Envelope>(body) {
        return CliError::Api {
            status: status.as_u16(),
            code: env.error.code,
            message: env.error.message,
            details: env.error.details,
        };
    }
    let text = body.trim();
    let message = if text.is_empty() {
        format!(
            "HTTP {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        )
        .trim()
        .to_owned()
    } else {
        format!("HTTP {}: {}", status.as_u16(), text)
    };
    CliError::Api {
        status: status.as_u16(),
        code: status
            .canonical_reason()
            .unwrap_or("error")
            .to_lowercase()
            .replace(' ', "_"),
        message,
        details: Default::default(),
    }
}

/// Lines for stderr that a client produced while it worked. The client has
/// no handle on stderr; whoever owns one prints and empties these.
pub type Notices = Arc<Mutex<Vec<String>>>;

/// Ask the server a write arrives at to carry it to the leader itself and
/// hold the answer until its own store has applied the write. Without this,
/// a write answered by the leader leaves the server `pi` was configured with
/// behind, and the next command — run milliseconds later against that same
/// server — reads there and finds nothing. Reads (GET and HEAD) never carry
/// it: a read belongs on the server it was asked of.
const FORWARD_HEADER: &str = "x-pistack-forward";
const FORWARD_LEADER: &str = "leader";

fn is_read(method: &Method) -> bool {
    *method == Method::GET || *method == Method::HEAD
}

/// What a follower answers when the forward header is there but the body is
/// past what it will buffer. It names the leader in `details.leader` and
/// asks that the client bring the body there itself.
const MSG_NOT_LEADER: &str = "not the leader";
const MSG_BODY_TOO_LARGE: &str =
    "the request is too large to pass on to the leader; send it to the leader";

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
    notices: Notices,
    /// Carries every read and write to whichever server answered last, so a
    /// wait after a write keeps reading where the write happened.
    home: Arc<Mutex<Option<String>>>,
}

/// The leader a refusal names, if a request sent to `base` may be sent there
/// instead: a different server on the same scheme. The token goes with the
/// retry, so a server reached over https is never followed to an http address.
/// Two refusals name a leader: a follower asked to carry a write it does not
/// lead, and a follower whose buffer cannot hold the body it was handed.
fn named_leader(base: &str, err: &CliError) -> Option<String> {
    let CliError::Api {
        status,
        message,
        details,
        ..
    } = err
    else {
        return None;
    };
    let refused = match *status {
        409 if message == MSG_NOT_LEADER => true,
        400 if message == MSG_BODY_TOO_LARGE => true,
        _ => false,
    };
    if !refused {
        return None;
    }
    let leader = details
        .get("leader")?
        .as_str()?
        .trim()
        .trim_end_matches('/');
    let there = reqwest::Url::parse(leader).ok()?;
    let here = reqwest::Url::parse(base).ok()?;
    if there.scheme() != here.scheme() || there.origin() == here.origin() {
        return None;
    }
    Some(leader.to_owned())
}

impl Client {
    pub fn new(url: &str, token: Option<String>, insecure: bool) -> CliResult<Client> {
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(insecure)
            .connect_timeout(Duration::from_secs(10))
            .user_agent(format!("pi/{}", pistack_types::BUILD))
            .build()?;
        Ok(Client {
            http,
            base: url.trim_end_matches('/').to_owned(),
            token,
            notices: Notices::default(),
            home: Arc::new(Mutex::new(None)),
        })
    }

    /// Collect this client's notices in `notices`.
    pub fn with_notices(mut self, notices: Notices) -> Client {
        self.notices = notices;
        self
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            return path.to_owned();
        }
        format!("{}/{}", self.base, path.trim_start_matches('/'))
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.request_url(method, self.address(path))
    }

    /// The single place a request is built. A write carries the forward
    /// header; a read does not.
    fn request_url(&self, method: Method, url: String) -> reqwest::RequestBuilder {
        let mut r = self.http.request(method.clone(), url);
        if let Some(t) = &self.token {
            r = r.bearer_auth(t);
        }
        if !is_read(&method) {
            r = r.header(FORWARD_HEADER, FORWARD_LEADER);
        }
        r
    }

    /// The server `path` goes to: the leader this client was sent to, else
    /// the one it was configured with.
    fn address(&self, path: &str) -> String {
        let home = self
            .home
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_else(|| self.base.clone());
        if path.starts_with("http://") || path.starts_with("https://") {
            return path.to_owned();
        }
        format!("{home}/{}", path.trim_start_matches('/'))
    }

    /// Where the next request goes.
    #[cfg(test)]
    fn settled(&self) -> String {
        self.address("/")
    }

    /// Remember `leader` as the server to send the rest of this client's
    /// requests to. Only a leader named by a refusal, whose scheme matches
    /// the address given: a token is not handed to an address nobody chose.
    /// Nothing is remembered across commands or processes.
    fn settle(&self, leader: &str) {
        let mut held = self.home.lock().unwrap_or_else(|e| e.into_inner());
        if held.as_deref() != Some(leader) {
            *held = Some(leader.to_owned());
        }
    }

    /// Where to send `path` again after `err`, when a follower refused it
    /// and named the leader — either because it does not lead, or because
    /// the body is too large for it to carry. Says so on stderr, and the
    /// rest of this client's requests go there too. The caller retries once:
    /// a second refusal is returned as it is.
    fn leader_url(&self, path: &str, err: &CliError) -> Option<String> {
        if path.starts_with("http://") || path.starts_with("https://") {
            return None;
        }
        let leader = named_leader(&self.base, err)?;
        let notice = format!(
            "{} is not the leader; sending the request to {leader}",
            self.base
        );
        let mut notices = self.notices.lock().unwrap_or_else(|e| e.into_inner());
        if !notices.contains(&notice) {
            notices.push(notice);
        }
        self.settle(&leader);
        Some(format!("{leader}/{}", path.trim_start_matches('/')))
    }

    async fn send_json(
        &self,
        method: Method,
        url: String,
        body: Option<&Value>,
        if_match: Option<&str>,
    ) -> CliResult<(StatusCode, Value)> {
        let mut r = self.request_url(method, url.clone());
        if let Some(revision) = if_match {
            r = r.header(reqwest::header::IF_MATCH, revision);
        }
        if let Some(b) = body {
            r = r.json(b);
        }
        let resp = r
            .send()
            .await
            .map_err(|e| CliError::Transport(format!("{e} ({url})")))?;
        self.finish(resp).await
    }

    /// One JSON request, sent again at the leader when a follower refuses it.
    /// The body is a value held in memory, so the retry builds it again.
    /// The leader a follower names takes the rest of this client's requests,
    /// so a wait after a write reads where the write happened.
    async fn call_following(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        if_match: Option<&str>,
    ) -> CliResult<(StatusCode, Value)> {
        let first = self
            .send_json(method.clone(), self.address(path), body, if_match)
            .await;
        let Err(err) = &first else {
            return first;
        };
        match self.leader_url(path, err) {
            Some(url) => self.send_json(method, url, body, if_match).await,
            None => first,
        }
    }

    async fn finish(&self, resp: reqwest::Response) -> CliResult<(StatusCode, Value)> {
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(parse_error(status, &text));
        }
        if text.trim().is_empty() {
            return Ok((status, Value::Null));
        }
        let v: Value = serde_json::from_str(&text)?;
        Ok((status, v))
    }

    /// Send `method path` with an optional JSON body; the response as JSON
    /// (`Null` for empty bodies). A write that reaches a follower is sent
    /// once more, to the leader the follower names.
    pub async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> CliResult<(StatusCode, Value)> {
        self.call_following(method, path, body, None).await
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> CliResult<T> {
        let (_, v) = self.call(Method::GET, path, None).await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get_value(&self, path: &str) -> CliResult<Value> {
        Ok(self.call(Method::GET, path, None).await?.1)
    }

    /// `GET` that answers `None` on `not found` instead of an error: for a
    /// caller that is looking something up of its own accord and wants to keep
    /// its own words for a row that is not there. Any other failure, including
    /// a `forbidden` whose message names an object the caller never asked
    /// about, is returned as it came.
    pub async fn get_or_none(&self, path: &str) -> CliResult<Option<Value>> {
        match self.call(Method::GET, path, None).await {
            Ok((_, v)) => Ok(Some(v)),
            Err(CliError::Api { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn post(&self, path: &str, body: &Value) -> CliResult<Value> {
        Ok(self.call(Method::POST, path, Some(body)).await?.1)
    }

    pub async fn post_empty(&self, path: &str) -> CliResult<Value> {
        Ok(self.call(Method::POST, path, None).await?.1)
    }

    pub async fn put(&self, path: &str, body: &Value) -> CliResult<Value> {
        Ok(self.call(Method::PUT, path, Some(body)).await?.1)
    }

    /// PUT with an `If-Match` revision header (blueprint stack update).
    pub async fn put_if_match(&self, path: &str, body: &Value, revision: &str) -> CliResult<Value> {
        Ok(self
            .call_following(Method::PUT, path, Some(body), Some(revision))
            .await?
            .1)
    }

    pub async fn delete(&self, path: &str) -> CliResult<(StatusCode, Value)> {
        self.call(Method::DELETE, path, None).await
    }

    /// Stream a file as the request body (image upload). A follower's
    /// refusal is followed to the leader like [`Client::call`]: the stream
    /// the follower partly consumed is discarded and the file opened again,
    /// so the leader is handed the whole body and not the tail of it.
    pub async fn put_file(&self, path: &str, file: &Path) -> CliResult<Value> {
        let first = self.put_file_to(self.address(path), file).await;
        let Err(err) = &first else {
            return first;
        };
        match self.leader_url(path, err) {
            Some(url) => self.put_file_to(url, file).await,
            None => first,
        }
    }

    async fn put_file_to(&self, url: String, file: &Path) -> CliResult<Value> {
        let f = tokio::fs::File::open(file)
            .await
            .map_err(|e| CliError::usage(format!("opening {}: {e}", file.display())))?;
        let len = f.metadata().await?.len();
        let stream = tokio_util::io::ReaderStream::with_capacity(f, 256 * 1024);
        let body = reqwest::Body::wrap_stream(stream);
        let resp = self
            .request_url(Method::PUT, url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(reqwest::header::CONTENT_LENGTH, len)
            .body(body)
            .send()
            .await?;
        Ok(self.finish(resp).await?.1)
    }

    /// Stream a response body to a file (image download). Returns bytes written.
    pub async fn get_to_file(&self, path: &str, out: &Path) -> CliResult<u64> {
        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;
        let resp = self.open_get(path).await?;
        let mut file = tokio::fs::File::create(out)
            .await
            .map_err(|e| CliError::usage(format!("creating {}: {e}", out.display())))?;
        let mut stream = resp.bytes_stream();
        let mut n = 0u64;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            n += chunk.len() as u64;
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok(n)
    }

    /// Stream a response body straight to `out` (an object read to stdout).
    /// Returns bytes written.
    pub async fn get_to_writer(&self, path: &str, out: &mut dyn std::io::Write) -> CliResult<u64> {
        let resp = self.open_get(path).await?;
        pump(resp, out).await
    }

    /// A successful `GET`, or the error the server answered with.
    async fn open_get(&self, path: &str) -> CliResult<reqwest::Response> {
        let resp = self.request(Method::GET, path).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await?;
            return Err(parse_error(status, &text));
        }
        Ok(resp)
    }
}

/// Copy a response body into `out` a chunk at a time.
async fn pump(resp: reqwest::Response, out: &mut dyn std::io::Write) -> CliResult<u64> {
    use futures::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut n = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        n += chunk.len() as u64;
        out.write_all(&chunk)?;
    }
    out.flush()?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_envelope() {
        let body = r#"{"error":{"code":"not_found","message":"machine 'web' not found","details":{"id":"x"}}}"#;
        let e = parse_error(StatusCode::NOT_FOUND, body);
        match e {
            CliError::Api {
                status,
                code,
                message,
                details,
            } => {
                assert_eq!(status, 404);
                assert_eq!(code, "not_found");
                assert_eq!(message, "machine 'web' not found");
                assert_eq!(details["id"], "x");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn details_default_when_absent() {
        let body = r#"{"error":{"code":"unauthorized","message":"invalid credentials"}}"#;
        let e = parse_error(StatusCode::UNAUTHORIZED, body);
        assert!(e.is_unauthorized());
        assert_eq!(e.to_string(), "invalid credentials");
    }

    #[test]
    fn falls_back_to_raw_text() {
        let e = parse_error(StatusCode::BAD_REQUEST, "Failed to parse the request body");
        assert_eq!(e.to_string(), "HTTP 400: Failed to parse the request body");
        let e = parse_error(StatusCode::BAD_GATEWAY, "");
        assert_eq!(e.to_string(), "HTTP 502 Bad Gateway");
    }

    #[test]
    fn a_named_leader_is_followed_only_on_the_same_scheme() {
        let refusal = |leader: Value| {
            let mut details = serde_json::Map::new();
            details.insert("leader".into(), leader);
            CliError::Api {
                status: 409,
                code: "conflict".into(),
                message: "not the leader".into(),
                details,
            }
        };
        let base = "http://10.0.0.12:8443";
        assert_eq!(
            named_leader(base, &refusal("http://10.0.0.13:8443/".into())).as_deref(),
            Some("http://10.0.0.13:8443")
        );
        // Never from https to http, or the other way.
        assert_eq!(
            named_leader(
                "https://10.0.0.12:8444",
                &refusal("http://10.0.0.13:8443".into())
            ),
            None
        );
        assert_eq!(
            named_leader(base, &refusal("https://10.0.0.13:8444".into())),
            None
        );
        // A follower that names itself, no leader, or something that is
        // not an address.
        assert_eq!(named_leader(base, &refusal(base.into())), None);
        assert_eq!(named_leader(base, &refusal("".into())), None);
        assert_eq!(named_leader(base, &refusal("10.0.0.13".into())), None);
        assert_eq!(named_leader(base, &refusal(Value::Null)), None);
        // Any other conflict is not a redirect.
        let mut details = serde_json::Map::new();
        details.insert("leader".into(), "http://10.0.0.13:8443".into());
        let other = CliError::Api {
            status: 409,
            code: "conflict".into(),
            message: "machine already exists".into(),
            details,
        };
        assert_eq!(named_leader(base, &other), None);
    }

    #[test]
    fn a_client_keeps_the_leader_it_was_told_about() {
        let c = Client::new("http://10.0.0.12:8443", None, false).unwrap();
        assert_eq!(c.settled(), "http://10.0.0.12:8443/");
        c.settle("http://10.0.0.13:8443");
        assert_eq!(c.settled(), "http://10.0.0.13:8443/");
        // A path that is already an address is sent where it says.
        assert_eq!(c.address("http://x/y"), "http://x/y");
    }

    #[tokio::test]
    async fn the_server_that_refused_a_write_takes_what_comes_next() {
        use axum::Json;
        use serde_json::json;
        async fn refuse(leader: String) -> (String, Arc<Mutex<u32>>) {
            let hits = Arc::new(Mutex::new(0));
            let count = hits.clone();
            let app = axum::Router::new().fallback(move || {
                let leader = leader.clone();
                let count = count.clone();
                async move {
                    *count.lock().unwrap() += 1;
                    (
                        StatusCode::CONFLICT,
                        Json(
                            json!({"error": {"code": "conflict", "message": "not the leader",
                            "details": {"leader": leader}}}),
                        ),
                    )
                }
            });
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
            (format!("http://{addr}"), hits)
        }
        let (leader, leader_hits) = refuse(String::new()).await;
        let (url, refused) = refuse(leader.clone()).await;
        let c = Client::new(&url, Some("t".into()), false).unwrap();
        let out = c.post("/compute/machines", &json!({"name": "web"})).await;
        assert!(out.is_err(), "the leader's own refusal comes back");
        assert_eq!(*refused.lock().unwrap(), 1);
        assert_eq!(*leader_hits.lock().unwrap(), 1);
        // The reads that follow go to the leader the write reached.
        assert_eq!(c.settled(), format!("{leader}/"));
        let _ = c.get_value("/compute/machines/mach-1").await;
        assert_eq!(*leader_hits.lock().unwrap(), 2);
        assert_eq!(*refused.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_write_asks_the_server_it_reached_to_carry_it_to_the_leader() {
        use axum::extract::Request;
        use axum::http::HeaderMap;
        use serde_json::json;
        async fn echo_headers() -> (String, Arc<Mutex<Vec<(String, Option<String>)>>>) {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let log = seen.clone();
            let app = axum::Router::new().fallback(move |req: Request| {
                let log = log.clone();
                async move {
                    let method = req.method().to_string();
                    let header = req
                        .headers()
                        .get(FORWARD_HEADER)
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v.to_owned());
                    log.lock().unwrap().push((method, header));
                    (HeaderMap::new(), axum::Json(json!({"ok": true})))
                }
            });
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
            (format!("http://{addr}"), seen)
        }
        let (url, seen) = echo_headers().await;
        let c = Client::new(&url, Some("t".into()), false).unwrap();
        let body = json!({"name": "x"});
        c.post("/alarms/alarms", &body).await.unwrap();
        c.put("/alarms/alarms/a-1", &body).await.unwrap();
        c.call(Method::PATCH, "/alarms/alarms/a-1", Some(&body))
            .await
            .unwrap();
        c.delete("/alarms/alarms/a-1").await.unwrap();
        let _ = c.get_value("/alarms/alarms").await.unwrap();
        let head = c.request(Method::HEAD, "/alarms/alarms").send().await;
        let _ = head;
        let log = seen.lock().unwrap();
        let carried = |method: &str| {
            log.iter()
                .find(|(m, _)| m == method)
                .map(|(_, h)| h.as_deref())
        };
        for w in ["POST", "PUT", "PATCH", "DELETE"] {
            assert_eq!(carried(w), Some(Some(FORWARD_LEADER)), "{w} carries it");
        }
        assert_eq!(carried("GET"), Some(None), "a read does not");
        assert_eq!(carried("HEAD"), Some(None), "a head read does not");
    }

    #[test]
    fn a_upload_too_large_to_carry_is_followed_too() {
        let refusal = |status: u16, message: &str| {
            let mut details = serde_json::Map::new();
            details.insert("leader".into(), "http://10.0.0.13:8443".into());
            CliError::Api {
                status,
                code: if status == 409 {
                    "conflict"
                } else {
                    "bad_request"
                }
                .into(),
                message: message.into(),
                details,
            }
        };
        let base = "http://10.0.0.12:8443";
        assert_eq!(
            named_leader(base, &refusal(400, MSG_BODY_TOO_LARGE)).as_deref(),
            Some("http://10.0.0.13:8443")
        );
        // Any other 400 that happens to name a leader is a complaint about
        // the request, not a pointer at the leader.
        assert_eq!(
            named_leader(base, &refusal(400, "the key is not valid")),
            None
        );
        assert_eq!(
            named_leader(base, &refusal(400, "the request is too large")),
            None
        );
        // The same checks as the 409: same scheme, and a different server.
        assert_eq!(
            named_leader("https://10.0.0.12:8444", &refusal(400, MSG_BODY_TOO_LARGE)),
            None
        );
        let mut itself = refusal(400, MSG_BODY_TOO_LARGE);
        if let CliError::Api { details, .. } = &mut itself {
            details.insert("leader".into(), base.into());
        }
        assert_eq!(named_leader(base, &itself), None);
        // Too large for a server that does not name one.
        let no_leader = CliError::Api {
            status: 400,
            code: "bad_request".into(),
            message: MSG_BODY_TOO_LARGE.into(),
            details: Default::default(),
        };
        assert_eq!(named_leader(base, &no_leader), None);
        // A refusal of that shape at another status is nobody's redirect.
        assert_eq!(named_leader(base, &refusal(413, MSG_BODY_TOO_LARGE)), None);
    }

    #[tokio::test]
    async fn an_upload_a_follower_cannot_carry_is_handed_to_the_leader_whole() {
        use axum::body::Bytes;
        use serde_json::json;
        use sha2::{Digest, Sha256};
        // A file of several chunks, so a stream that is only partly consumed
        // cannot pass as a whole body.
        let mut bytes = Vec::with_capacity(600 * 1024);
        for i in 0..(600 * 1024) {
            bytes.push((i % 251) as u8);
        }
        let digest = Sha256::digest(&bytes);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("payload");
        std::fs::write(&file, &bytes).unwrap();

        let seen_at = |bytes: Bytes| {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            hasher.finalize()
        };

        // The leader: keeps every byte it is handed.
        let leader_got: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let log = leader_got.clone();
        let leader_app = axum::Router::new().fallback(move |body: Bytes| {
            let log = log.clone();
            async move {
                *log.lock().unwrap() = seen_at(body).to_vec();
                axum::Json(json!({"key": "payload"}))
            }
        });
        let leader_l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let leader_addr = leader_l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(leader_l, leader_app).await.unwrap() });

        // The follower: refuses a body it cannot carry, naming the leader.
        let follower_hits = Arc::new(Mutex::new(0));
        let count = follower_hits.clone();
        let follower_app = axum::Router::new().fallback(move |body: Bytes| {
            let count = count.clone();
            let leader = format!("http://{leader_addr}");
            async move {
                *count.lock().unwrap() += 1;
                // Whatever arrives is read to the end right here and then
                // dropped: the bytes the leader receives must come from the
                // file, not from a stream this server already swallowed.
                let swallowed = body.len();
                assert!(swallowed > 0, "the follower was handed a body");
                (
                    StatusCode::BAD_REQUEST,
                    axum::Json(
                        json!({"error": {"code": "bad_request",
                        "message": "the request is too large to pass on to the leader; send it to the leader",
                        "details": {"leader": leader}}}),
                    ),
                )
            }
        });
        let follower_l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let follower_addr = follower_l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(follower_l, follower_app).await.unwrap() });

        let notices = Notices::default();
        let c = Client::new(&format!("http://{follower_addr}"), Some("t".into()), false)
            .unwrap()
            .with_notices(notices.clone());
        let v = c.put_file("/silo/buckets/b/objects/payload", &file).await;
        assert_eq!(v.unwrap()["key"], "payload");
        assert_eq!(*follower_hits.lock().unwrap(), 1, "asked once");
        assert_eq!(
            leader_got.lock().unwrap().as_slice(),
            digest.as_slice(),
            "the leader received every byte"
        );
        let said = notices.lock().unwrap().clone();
        assert_eq!(
            said,
            vec![format!(
                "http://{follower_addr} is not the leader; sending the request to http://{leader_addr}"
            )],
            "said once where the upload went"
        );
        assert_eq!(c.settled(), format!("http://{leader_addr}/"));
    }

    #[test]
    fn url_joins_once() {
        let c = Client::new("http://h:1/", None, false).unwrap();
        assert_eq!(c.url("/compute/machines"), "http://h:1/compute/machines");
        assert_eq!(c.url("compute/machines"), "http://h:1/compute/machines");
        assert_eq!(c.url("http://x/y"), "http://x/y");
    }
}
