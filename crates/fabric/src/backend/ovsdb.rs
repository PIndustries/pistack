// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A small client for the OVSDB management protocol (RFC 7047), enough for
//! fabric to write OVN's northbound database: one `transact` per call, over
//! a fresh TCP or unix socket connection, with a timeout.
//!
//! Also the JSON helpers used to build operations, and (in tests) an
//! in-memory database that applies those operations so backend tests can
//! check real end states.

use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Runs one transaction. One result per operation, in order.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    async fn transact(&self, db: &str, ops: Vec<Value>) -> ApiResult<Vec<Value>>;
}

/// Where the database listens: `tcp:<host>:<port>` or `unix:<path>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    Tcp(String),
    Unix(std::path::PathBuf),
}

impl Endpoint {
    pub fn parse(s: &str) -> ApiResult<Self> {
        let s = s.trim();
        if let Some(a) = s.strip_prefix("tcp:").filter(|a| !a.is_empty()) {
            return Ok(Self::Tcp(a.to_string()));
        }
        if let Some(p) = s.strip_prefix("unix:").filter(|p| !p.is_empty()) {
            return Ok(Self::Unix(p.into()));
        }
        Err(ApiError::bad_request(format!(
            "ovsdb endpoint {s:?}: want tcp:<host>:<port> or unix:<path>"
        )))
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp(a) => write!(f, "tcp:{a}"),
            Self::Unix(p) => write!(f, "unix:{}", p.display()),
        }
    }
}

/// Connects per transaction. Cheap next to the database itself, and there
/// is no session state to lose when the database restarts.
#[derive(Clone, Debug)]
pub struct Client {
    endpoint: Endpoint,
    timeout: Duration,
}

impl Client {
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            timeout: Duration::from_secs(10),
        }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

fn io_err(e: std::io::Error) -> ApiError {
    ApiError::unavailable(format!("ovsdb: {e}"))
}

#[async_trait]
impl Transport for Client {
    async fn transact(&self, db: &str, ops: Vec<Value>) -> ApiResult<Vec<Value>> {
        let run = async {
            match &self.endpoint {
                Endpoint::Tcp(a) => {
                    let s = tokio::net::TcpStream::connect(a).await.map_err(io_err)?;
                    exchange(s, db, ops).await
                }
                Endpoint::Unix(p) => {
                    let s = tokio::net::UnixStream::connect(p).await.map_err(io_err)?;
                    exchange(s, db, ops).await
                }
            }
        };
        tokio::time::timeout(self.timeout, run)
            .await
            .map_err(|_| ApiError::unavailable(format!("ovsdb {}: timed out", self.endpoint)))?
    }
}

/// Send one `transact` and read until its reply, answering `echo` on the way.
async fn exchange<S>(mut s: S, db: &str, ops: Vec<Value>) -> ApiResult<Vec<Value>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let want = ops.len();
    let mut params = Vec::with_capacity(want + 1);
    params.push(Value::from(db));
    params.extend(ops);
    let req = json!({"method": "transact", "params": params, "id": 1});
    s.write_all(req.to_string().as_bytes())
        .await
        .map_err(io_err)?;
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let mut msgs = Vec::new();
        let mut used = 0;
        {
            let mut it = serde_json::Deserializer::from_slice(&buf).into_iter::<Value>();
            loop {
                match it.next() {
                    Some(Ok(m)) => {
                        used = it.byte_offset();
                        msgs.push(m);
                    }
                    Some(Err(e)) if e.is_eof() => break,
                    Some(Err(e)) => {
                        return Err(ApiError::unavailable(format!("ovsdb: bad reply: {e}")))
                    }
                    None => break,
                }
            }
        }
        buf.drain(..used);
        for m in msgs {
            if m.get("method").and_then(Value::as_str) == Some("echo") {
                let reply = json!({"result": m["params"], "error": null, "id": m["id"]});
                s.write_all(reply.to_string().as_bytes())
                    .await
                    .map_err(io_err)?;
                continue;
            }
            if m.get("id") == Some(&json!(1)) {
                return reply_results(&m, want);
            }
        }
        let n = s.read(&mut chunk).await.map_err(io_err)?;
        if n == 0 {
            return Err(ApiError::unavailable(
                "ovsdb: connection closed before reply",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Turn a `transact` reply into per-op results, or the first error. A
/// failed commit adds one extra result past the operations.
fn reply_results(m: &Value, want: usize) -> ApiResult<Vec<Value>> {
    if let Some(e) = m.get("error").filter(|e| !e.is_null()) {
        return Err(ApiError::unavailable(format!("ovsdb: {e}")));
    }
    let results = m
        .get("result")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::unavailable("ovsdb: reply has no result"))?;
    if let Some(err) = results.iter().find_map(op_error) {
        return Err(err);
    }
    if results.len() < want {
        return Err(ApiError::unavailable("ovsdb: reply is short"));
    }
    Ok(results[..want].to_vec())
}

fn op_error(r: &Value) -> Option<ApiError> {
    let e = r.get("error")?.as_str()?;
    let details = r.get("details").and_then(Value::as_str).unwrap_or("");
    Some(ApiError::unavailable(format!("ovsdb: {e}: {details}")).with_detail("ovsdb_error", e))
}

// ---- values ----------------------------------------------------------------

/// `["map", [[k, v], ...]]` with string keys and values.
pub fn map<K: ToString, V: ToString>(pairs: impl IntoIterator<Item = (K, V)>) -> Value {
    let pairs: Vec<Value> = pairs
        .into_iter()
        .map(|(k, v)| json!([k.to_string(), v.to_string()]))
        .collect();
    json!(["map", pairs])
}

/// `["set", [...]]`.
pub fn set(items: impl IntoIterator<Item = Value>) -> Value {
    json!(["set", items.into_iter().collect::<Vec<_>>()])
}

pub fn uuid(u: &str) -> Value {
    json!(["uuid", u])
}

pub fn named(n: &str) -> Value {
    json!(["named-uuid", n])
}

pub fn eq(col: &str, v: Value) -> Value {
    json!([col, "==", v])
}

pub fn includes(col: &str, v: Value) -> Value {
    json!([col, "includes", v])
}

// ---- operations ------------------------------------------------------------

pub fn select(table: &str, cond: Vec<Value>) -> Value {
    json!({"op": "select", "table": table, "where": cond})
}

pub fn insert(table: &str, row: Value, name: &str) -> Value {
    json!({"op": "insert", "table": table, "row": row, "uuid-name": name})
}

pub fn update(table: &str, cond: Vec<Value>, row: Value) -> Value {
    json!({"op": "update", "table": table, "where": cond, "row": row})
}

pub fn mutate(table: &str, cond: Vec<Value>, mutations: Vec<Value>) -> Value {
    json!({"op": "mutate", "table": table, "where": cond, "mutations": mutations})
}

pub fn delete(table: &str, cond: Vec<Value>) -> Value {
    json!({"op": "delete", "table": table, "where": cond})
}

/// Abort the transaction unless no row matches `cond`. Guards an insert
/// against a twin written since our select.
pub fn absent(table: &str, cond: Vec<Value>) -> Value {
    json!({"op": "wait", "table": table, "where": cond, "columns": ["_uuid"],
           "until": "==", "rows": [], "timeout": 0})
}

/// Abort the transaction unless some row matches `cond`.
pub fn present(table: &str, cond: Vec<Value>) -> Value {
    json!({"op": "wait", "table": table, "where": cond, "columns": ["_uuid"],
           "until": "!=", "rows": [], "timeout": 0})
}

// ---- reading results -------------------------------------------------------

pub fn rows(result: &Value) -> Vec<Value> {
    result
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub fn row_uuid(row: &Value) -> Option<String> {
    let u = row.get("_uuid")?.as_array()?;
    if u.first()?.as_str()? != "uuid" {
        return None;
    }
    u.get(1)?.as_str().map(str::to_string)
}

/// Elements of a set column; an atom is a one-element set.
pub fn elems(v: Option<&Value>) -> Vec<Value> {
    match v {
        None => Vec::new(),
        Some(v) => match v.as_array() {
            Some(a) if a.len() == 2 && a[0] == "set" => {
                a[1].as_array().cloned().unwrap_or_default()
            }
            _ => vec![v.clone()],
        },
    }
}

/// A canonical form for comparing column values: one-element sets become
/// their atom, sets and maps are sorted.
pub fn canon(v: &Value) -> Value {
    match v.as_array() {
        Some(a) if a.len() == 2 && a[0] == "set" => {
            let mut items: Vec<Value> = a[1]
                .as_array()
                .map(|xs| xs.iter().map(canon).collect())
                .unwrap_or_default();
            if items.len() == 1 {
                return items.remove(0);
            }
            items.sort_by_key(|x| x.to_string());
            json!(["set", items])
        }
        Some(a) if a.len() == 2 && a[0] == "map" => {
            let mut pairs: Vec<Value> = a[1].as_array().cloned().unwrap_or_default();
            pairs.sort_by_key(|x| x.to_string());
            json!(["map", pairs])
        }
        _ => v.clone(),
    }
}

/// True when every column of `want` already has that value in `row`.
pub fn row_matches(row: &Value, want: &Value) -> bool {
    let Some(want) = want.as_object() else {
        return false;
    };
    want.iter().all(|(col, v)| {
        let have = row
            .get(col)
            .map(canon)
            .unwrap_or_else(|| json!(["set", []]));
        have == canon(v)
    })
}

#[cfg(test)]
pub(crate) mod fake {
    //! An in-memory database applying the operations above, with the one
    //! piece of garbage collection fabric relies on: switch ports nobody
    //! references disappear at commit, as they do in OVN's schema.

    use super::*;
    use parking_lot::Mutex;
    use serde_json::Map;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct State {
        tables: BTreeMap<String, Vec<Map<String, Value>>>,
        next: u64,
    }

    #[derive(Default)]
    pub struct FakeDb {
        state: Mutex<State>,
        log: Mutex<Vec<Vec<Value>>>,
    }

    impl FakeDb {
        pub fn new() -> Self {
            Self::default()
        }

        /// Rows of `table`.
        pub fn rows(&self, table: &str) -> Vec<Value> {
            let st = self.state.lock();
            st.tables
                .get(table)
                .map(|rs| rs.iter().map(|r| Value::Object(r.clone())).collect())
                .unwrap_or_default()
        }

        /// Committed transactions that changed something.
        pub fn writes(&self) -> usize {
            self.log
                .lock()
                .iter()
                .filter(|ops| ops.iter().any(|o| o["op"] != "select" && o["op"] != "wait"))
                .count()
        }
    }

    fn resolve(v: &Value, names: &BTreeMap<String, String>) -> Value {
        match v {
            Value::Array(a) if a.len() == 2 && a[0] == "named-uuid" => {
                let n = a[1].as_str().unwrap_or("");
                names.get(n).map(|u| uuid(u)).unwrap_or_else(|| v.clone())
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| resolve(x, names)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| (k.clone(), resolve(x, names)))
                    .collect(),
            ),
            _ => v.clone(),
        }
    }

    fn pairs(v: &Value) -> Vec<(Value, Value)> {
        v.as_array()
            .filter(|a| a.len() == 2 && a[0] == "map")
            .and_then(|a| a[1].as_array())
            .map(|ps| ps.iter().map(|p| (p[0].clone(), p[1].clone())).collect())
            .unwrap_or_default()
    }

    fn is_map(v: &Value) -> bool {
        v.as_array().is_some_and(|a| a.len() == 2 && a[0] == "map")
    }

    fn holds(row: &Map<String, Value>, cond: &Value) -> bool {
        let col = cond[0].as_str().unwrap_or("");
        let f = cond[1].as_str().unwrap_or("");
        let want = &cond[2];
        let have = row.get(col).cloned().unwrap_or_else(|| json!(["set", []]));
        match f {
            "==" => canon(&have) == canon(want),
            "!=" => canon(&have) != canon(want),
            "includes" if is_map(want) => {
                let hp = pairs(&have);
                pairs(want).iter().all(|p| hp.contains(p))
            }
            "includes" => {
                let he: Vec<Value> = elems(Some(&have)).iter().map(canon).collect();
                elems(Some(want)).iter().all(|w| he.contains(&canon(w)))
            }
            other => panic!("fake ovsdb: condition {other}"),
        }
    }

    fn matching<'a>(
        st: &'a mut State,
        table: &str,
        cond: &Value,
    ) -> impl Iterator<Item = &'a mut Map<String, Value>> {
        let conds: Vec<Value> = cond.as_array().cloned().unwrap_or_default();
        st.tables
            .entry(table.to_string())
            .or_default()
            .iter_mut()
            .filter(move |r| conds.iter().all(|c| holds(r, c)))
    }

    fn apply_mutation(row: &mut Map<String, Value>, m: &Value) {
        let col = m[0].as_str().unwrap_or("").to_string();
        let how = m[1].as_str().unwrap_or("");
        let arg = &m[2];
        let cur = row.get(&col).cloned();
        let new = if is_map(arg) || cur.as_ref().is_some_and(is_map) {
            let mut ps = cur.as_ref().map(pairs).unwrap_or_default();
            match how {
                "insert" => {
                    for (k, v) in pairs(arg) {
                        if !ps.iter().any(|(pk, _)| *pk == k) {
                            ps.push((k, v));
                        }
                    }
                }
                "delete" if is_map(arg) => {
                    let gone = pairs(arg);
                    ps.retain(|p| !gone.contains(p));
                }
                "delete" => {
                    let keys = elems(Some(arg));
                    ps.retain(|(k, _)| !keys.contains(k));
                }
                other => panic!("fake ovsdb: mutator {other}"),
            }
            json!([
                "map",
                ps.into_iter()
                    .map(|(k, v)| json!([k, v]))
                    .collect::<Vec<_>>()
            ])
        } else {
            let mut es: Vec<Value> = elems(cur.as_ref());
            match how {
                "insert" => {
                    for e in elems(Some(arg)) {
                        if !es.contains(&e) {
                            es.push(e);
                        }
                    }
                }
                "delete" => {
                    let gone = elems(Some(arg));
                    es.retain(|e| !gone.contains(e));
                }
                other => panic!("fake ovsdb: mutator {other}"),
            }
            set(es)
        };
        row.insert(col, new);
    }

    fn collect_garbage(st: &mut State) {
        let referenced: Vec<Value> = st
            .tables
            .get("Logical_Switch")
            .map(|ls| ls.iter().flat_map(|r| elems(r.get("ports"))).collect())
            .unwrap_or_default();
        if let Some(lsps) = st.tables.get_mut("Logical_Switch_Port") {
            lsps.retain(|r| r.get("_uuid").is_some_and(|u| referenced.contains(u)));
        }
        // `Port_Group.ports` refers to switch ports weakly: a deleted
        // port just drops out of its groups.
        let live: Vec<Value> = st
            .tables
            .get("Logical_Switch_Port")
            .map(|ps| ps.iter().filter_map(|r| r.get("_uuid").cloned()).collect())
            .unwrap_or_default();
        if let Some(groups) = st.tables.get_mut("Port_Group") {
            for g in groups.iter_mut() {
                let ports = elems(g.get("ports"));
                if ports.iter().any(|p| !live.contains(p)) {
                    let kept: Vec<Value> = ports.into_iter().filter(|p| live.contains(p)).collect();
                    g.insert("ports".into(), set(kept));
                }
            }
        }
        // ACLs are not a root table: rows no group or switch points at go.
        let acls: Vec<Value> = ["Port_Group", "Logical_Switch"]
            .iter()
            .filter_map(|t| st.tables.get(*t))
            .flat_map(|rs| rs.iter().flat_map(|r| elems(r.get("acls"))))
            .collect();
        if let Some(rows) = st.tables.get_mut("ACL") {
            rows.retain(|r| r.get("_uuid").is_some_and(|u| acls.contains(u)));
        }
    }

    #[async_trait]
    impl Transport for FakeDb {
        async fn transact(&self, db: &str, ops: Vec<Value>) -> ApiResult<Vec<Value>> {
            assert_eq!(db, "OVN_Northbound");
            let mut guard = self.state.lock();
            let mut st = State {
                tables: guard.tables.clone(),
                next: guard.next,
            };
            let mut names = BTreeMap::new();
            let mut out = Vec::new();
            for op in &ops {
                let op = resolve(op, &names);
                let table = op["table"].as_str().unwrap_or("").to_string();
                let res = match op["op"].as_str().unwrap_or("") {
                    "select" => {
                        let rs: Vec<Value> = matching(&mut st, &table, &op["where"])
                            .map(|r| Value::Object(r.clone()))
                            .collect();
                        json!({"rows": rs})
                    }
                    "insert" => {
                        st.next += 1;
                        let u = format!("00000000-0000-0000-0000-{:012}", st.next);
                        let mut row = op["row"].as_object().cloned().unwrap_or_default();
                        row.insert("_uuid".into(), uuid(&u));
                        st.tables.entry(table).or_default().push(row);
                        if let Some(n) = op["uuid-name"].as_str() {
                            names.insert(n.to_string(), u.clone());
                        }
                        json!({"uuid": uuid(&u)})
                    }
                    "update" => {
                        let cols = op["row"].as_object().cloned().unwrap_or_default();
                        let mut n = 0;
                        for r in matching(&mut st, &table, &op["where"]) {
                            for (k, v) in &cols {
                                r.insert(k.clone(), v.clone());
                            }
                            n += 1;
                        }
                        json!({"count": n})
                    }
                    "mutate" => {
                        let ms = op["mutations"].as_array().cloned().unwrap_or_default();
                        let mut n = 0;
                        for r in matching(&mut st, &table, &op["where"]) {
                            for m in &ms {
                                apply_mutation(r, m);
                            }
                            n += 1;
                        }
                        json!({"count": n})
                    }
                    "delete" => {
                        let conds = op["where"].as_array().cloned().unwrap_or_default();
                        let rows = st.tables.entry(table).or_default();
                        let before = rows.len();
                        rows.retain(|r| !conds.iter().all(|c| holds(r, c)));
                        json!({"count": before - rows.len()})
                    }
                    "wait" => {
                        let n = matching(&mut st, &table, &op["where"]).count();
                        let ok = match op["until"].as_str() {
                            Some("==") => n == 0,
                            Some("!=") => n > 0,
                            _ => panic!("fake ovsdb: wait shape"),
                        };
                        if !ok {
                            return Err(ApiError::unavailable("ovsdb: timed out: wait"));
                        }
                        json!({})
                    }
                    other => panic!("fake ovsdb: op {other}"),
                };
                out.push(res);
            }
            collect_garbage(&mut st);
            *guard = st;
            self.log.lock().push(ops);
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_parse() {
        assert_eq!(
            Endpoint::parse("tcp:127.0.0.1:6641").unwrap(),
            Endpoint::Tcp("127.0.0.1:6641".into())
        );
        assert_eq!(
            Endpoint::parse("unix:/var/run/ovn/ovnnb_db.sock").unwrap(),
            Endpoint::Unix("/var/run/ovn/ovnnb_db.sock".into())
        );
        assert!(Endpoint::parse("ssl:1.2.3.4:6641").is_err());
        assert!(Endpoint::parse("tcp:").is_err());
    }

    #[test]
    fn values_compare_canonically() {
        assert_eq!(canon(&set([json!("a")])), json!("a"));
        assert_eq!(
            canon(&map([("b", "2"), ("a", "1")])),
            canon(&map([("a", "1"), ("b", "2")]))
        );
        let row = json!({"name": "x", "addresses": ["set", ["m 1"]]});
        assert!(row_matches(&row, &json!({"addresses": "m 1"})));
        assert!(!row_matches(&row, &json!({"name": "y"})));
        assert!(row_matches(&row, &json!({"port_security": ["set", []]})));
    }

    /// The real client against a socket speaking the protocol: it answers
    /// an echo first, then the reply arrives split across two writes.
    #[tokio::test]
    async fn the_client_answers_echo_and_reads_a_split_reply() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap();
            let req: Value = serde_json::from_slice(&buf[..n]).unwrap();
            assert_eq!(req["method"], "transact");
            assert_eq!(req["params"][0], "OVN_Northbound");
            s.write_all(br#"{"method":"echo","params":["hi"],"id":"e"}"#)
                .await
                .unwrap();
            let n = s.read(&mut buf).await.unwrap();
            let echo: Value = serde_json::from_slice(&buf[..n]).unwrap();
            assert_eq!(echo["result"], json!(["hi"]));
            s.write_all(br#"{"id":1,"error":null,"res"#).await.unwrap();
            s.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
            s.write_all(br#"ult":[{"rows":[]}]}"#).await.unwrap();
        });
        let client = Client::new(Endpoint::Tcp(addr.to_string()));
        let out = client
            .transact("OVN_Northbound", vec![select("Logical_Switch", vec![])])
            .await
            .unwrap();
        assert_eq!(out, vec![json!({"rows": []})]);
        server.await.unwrap();
    }

    #[test]
    fn an_operation_error_fails_the_call() {
        let reply = json!({"id": 1, "error": null, "result": [
            {"count": 1},
            {"error": "constraint violation", "details": "duplicate name"}
        ]});
        let err = reply_results(&reply, 1).unwrap_err();
        assert!(err.to_string().contains("constraint violation"), "{err}");
    }
}
