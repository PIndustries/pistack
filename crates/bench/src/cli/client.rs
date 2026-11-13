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

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
    notices: Notices,
}

/// The leader a "not the leader" answer names, if a request sent to `base`
/// may be sent there instead: a different server on the same scheme. The
/// token goes with the retry, so a server reached over https is never
/// followed to an http address.
fn named_leader(base: &str, err: &CliError) -> Option<String> {
    let CliError::Api {
        status: 409,
        message,
        details,
        ..
    } = err
    else {
        return None;
    };
    if message != "not the leader" {
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
        self.request_url(method, self.url(path))
    }

    fn request_url(&self, method: Method, url: String) -> reqwest::RequestBuilder {
        let mut r = self.http.request(method, url);
        if let Some(t) = &self.token {
            r = r.bearer_auth(t);
        }
        r
    }

    /// Where to send `path` again after `err`, when a follower refused it
    /// and named the leader. Says so on stderr. The caller retries once: a
    /// second refusal is returned as it is.
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
    async fn call_following(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        if_match: Option<&str>,
    ) -> CliResult<(StatusCode, Value)> {
        let first = self
            .send_json(method.clone(), self.url(path), body, if_match)
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
    /// refusal is followed to the leader like [`Client::call`], reading the
    /// file again from the start.
    pub async fn put_file(&self, path: &str, file: &Path) -> CliResult<Value> {
        let first = self.put_file_to(self.url(path), file).await;
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
        let resp = self.request(Method::GET, path).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await?;
            return Err(parse_error(status, &text));
        }
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
    fn url_joins_once() {
        let c = Client::new("http://h:1/", None, false).unwrap();
        assert_eq!(c.url("/compute/machines"), "http://h:1/compute/machines");
        assert_eq!(c.url("compute/machines"), "http://h:1/compute/machines");
        assert_eq!(c.url("http://x/y"), "http://x/y");
    }
}
