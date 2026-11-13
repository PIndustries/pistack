// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Control-plane health as gauge series, and the `/metrics` endpoint a
//! Prometheus server scrapes.
//!
//! Each server answers for itself: its role, term and log, its store, what
//! its modules failed to reconcile and what it has queued for agents. The
//! leader also answers for the hosts and machines, because the agents
//! report to it. So every server is scraped, not one.
//!
//! The figures are read from what the cluster, the queue and the store
//! already expose. They are recorded into gauge every [`RECORD_EVERY`], so
//! an alarm can watch them, and once more when `/metrics` is asked.

use crate::cluster::ClusterHandle;
use axum::{
    extract::State,
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
};
use gauge::catalog as names;
use pistack_cluster::{Member, Role};
use pistack_rpc::CommandQueue;
use pistack_store::{Store, TableStats};
use pistack_types::ApiError;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A bearer token that may read `/metrics` and nothing else. Unset, only a
/// cloud admin's token is accepted.
pub const SCRAPE_TOKEN_ENV: &str = "PISTACK_METRICS_TOKEN";
/// A shorter scrape token is refused at start: it would be guessable.
const SCRAPE_TOKEN_MIN: usize = 16;
const RECORD_EVERY: Duration = Duration::from_secs(15);
/// Counting the rows of every table reads the whole store.
const TABLES_EVERY: Duration = Duration::from_secs(300);
/// The modules that count their failed reconciles, so each has a series
/// from the start and not only after its first failure.
const RECONCILERS: [&str; 7] = [
    "beacon", "fabric", "forge", "loom", "relay", "shelf", "slab",
];

/// What this server can say about itself.
pub struct ControlPlane {
    pub cluster: Option<Arc<ClusterHandle>>,
    pub queue: CommandQueue,
    pub store: Store,
    /// The store file, for its size. `None` for a store in memory.
    pub db_path: Option<PathBuf>,
    tables: Mutex<Vec<TableStats>>,
}

impl ControlPlane {
    pub fn new(
        cluster: Option<Arc<ClusterHandle>>,
        queue: CommandQueue,
        store: Store,
        db_path: Option<PathBuf>,
    ) -> Self {
        Self {
            cluster,
            queue,
            store,
            db_path,
            tables: Mutex::new(Vec::new()),
        }
    }

    /// Count the rows and bytes of each table. Slow on a large store.
    fn refresh_tables(&self) {
        match self.store.stats() {
            Ok(stats) => *lock(&self.tables) = stats,
            Err(e) => tracing::debug!(error = %e, "metrics: store tables not counted"),
        }
    }

    /// The series as they stand now.
    pub fn samples(&self) -> Vec<(String, f64)> {
        let mut out = vec![(
            gauge::series(names::BUILD_INFO, &[("build", pistack_types::BUILD)]),
            1.0,
        )];
        let commit = self.store.commit_index().ok();
        if let Some(handle) = &self.cluster {
            self.cluster_samples(&mut out, handle, commit);
        }

        let mut failures: std::collections::BTreeMap<&str, u64> =
            RECONCILERS.iter().map(|m| (*m, 0)).collect();
        failures.extend(pistack_types::tally::reconcile_failures());
        for (module, count) in failures {
            out.push((
                gauge::series(names::RECONCILE_FAILURES, &[("module", module)]),
                count as f64,
            ));
        }

        for node in self.queue.nodes() {
            let on_host = [("host", node.name.as_str())];
            out.push((
                gauge::series(names::AGENT_QUEUED, &on_host),
                node.queued as f64,
            ));
            out.push((
                gauge::series(names::AGENT_IN_FLIGHT, &on_host),
                node.in_flight as f64,
            ));
        }

        if let Some(size) = self
            .db_path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok())
        {
            out.push((names::STORE_FILE_BYTES.to_owned(), size.len() as f64));
        }
        if let (Some(commit), Ok(start)) = (commit, self.store.log_start()) {
            // Rows `start..=commit` are held; none before the first write.
            let held = if commit == 0 {
                0
            } else {
                (commit + 1).saturating_sub(start)
            };
            out.push((names::STORE_LOG_ENTRIES.to_owned(), held as f64));
        }
        for table in lock(&self.tables).iter() {
            let of_table = [("table", table.table.as_str())];
            out.push((
                gauge::series(names::STORE_TABLE_ROWS, &of_table),
                table.rows as f64,
            ));
            out.push((
                gauge::series(names::STORE_TABLE_BYTES, &of_table),
                (table.key_bytes + table.value_bytes) as f64,
            ));
        }
        out
    }

    fn cluster_samples(
        &self,
        out: &mut Vec<(String, f64)>,
        handle: &ClusterHandle,
        commit: Option<u64>,
    ) {
        let server = handle.node_id.as_str();
        let here = [("server", server)];
        let leads = handle.leader.load(Ordering::Acquire);
        let members: Vec<Member> = self
            .store
            .get_meta("members")
            .ok()
            .flatten()
            .unwrap_or_default();
        let role = members
            .iter()
            .find(|member| member.id == handle.node_id)
            .map_or_else(|| *lock(&handle.role), |member| member.role);
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        out.push((gauge::series(names::CLUSTER_LEADER, &here), flag(leads)));
        out.push((
            gauge::series(names::CLUSTER_VOTER, &here),
            flag(role == Role::Voter),
        ));
        out.push((
            gauge::series(names::CLUSTER_TERM, &here),
            handle.term.load(Ordering::Relaxed) as f64,
        ));
        if let Some(commit) = commit {
            out.push((gauge::series(names::CLUSTER_COMMIT, &here), commit as f64));
        }
        for (name, wanted) in [("voter", Role::Voter), ("learner", Role::Learner)] {
            let count = members.iter().filter(|m| m.role == wanted).count();
            out.push((
                gauge::series(
                    names::CLUSTER_MEMBERS,
                    &[("server", server), ("role", name)],
                ),
                count as f64,
            ));
        }
        // Only the leader is pulled from, so only it knows how far behind
        // the others are. A member that never pulled has no series.
        let (true, Some(commit)) = (leads, commit) else {
            return;
        };
        let progress = lock(&handle.progress).clone();
        for member in members.iter().filter(|m| m.id != handle.node_id) {
            let Some((after, at)) = progress.get(&member.id) else {
                continue;
            };
            let labels = [("server", server), ("member", member.id.as_str())];
            out.push((
                gauge::series(names::CLUSTER_FOLLOWER_LAG, &labels),
                commit.saturating_sub(*after) as f64,
            ));
            out.push((
                gauge::series(names::CLUSTER_FOLLOWER_PULL_AGE, &labels),
                at.elapsed().as_secs_f64(),
            ));
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Record this server's series every [`RECORD_EVERY`] until shutdown.
pub fn spawn(control: Arc<ControlPlane>, gauge: gauge::Client, shutdown: CancellationToken) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(RECORD_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut counted: Option<std::time::Instant> = None;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tick.tick() => {}
            }
            if counted.is_none_or(|at| at.elapsed() >= TABLES_EVERY) {
                counted = Some(std::time::Instant::now());
                let control = control.clone();
                let _ = tokio::task::spawn_blocking(move || control.refresh_tables()).await;
            }
            gauge.record_all(control.samples());
        }
    });
}

/// The scrape token from [`SCRAPE_TOKEN_ENV`]. One that is too short to be
/// a secret is not used, and said so.
pub fn scrape_token(value: Option<&str>) -> Option<Arc<str>> {
    let token = value.map(str::trim).filter(|t| !t.is_empty())?;
    if token.len() < SCRAPE_TOKEN_MIN {
        tracing::warn!(
            "{SCRAPE_TOKEN_ENV} is shorter than {SCRAPE_TOKEN_MIN} characters and is ignored; \
             /metrics takes a cloud admin's token only"
        );
        return None;
    }
    Some(Arc::from(token))
}

/// State of the `/metrics` route.
#[derive(Clone)]
pub struct Scrape {
    pub verifier: gate::Verifier,
    pub token: Option<Arc<str>>,
    /// Dev mode: every request is the admin, this one too.
    pub dev: bool,
    pub gauge: gauge::Client,
    pub control: Arc<ControlPlane>,
}

/// `GET /metrics`: everything this server knows, in the Prometheus text
/// format, for the scrape token or a cloud admin.
pub async fn scrape(State(s): State<Scrape>, headers: HeaderMap) -> Response {
    if let Err(refusal) = authorize(&s, &headers) {
        return refusal.into_response();
    }
    s.gauge.record_all(s.control.samples());
    gauge::api::text_response(s.gauge.metrics_text()).into_response()
}

fn authorize(s: &Scrape, headers: &HeaderMap) -> Result<(), ApiError> {
    if s.dev {
        return Ok(());
    }
    let needed = || ApiError::unauthorized("the scrape token or a cloud admin's token is required");
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return Err(needed());
    };
    let token = value
        .to_str()
        .ok()
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, token)| token.trim())
        .filter(|token| !token.is_empty())
        .ok_or_else(needed)?;
    if s.token
        .as_deref()
        .is_some_and(|expected| same(token, expected))
    {
        return Ok(());
    }
    match s.verifier.verify(token) {
        Ok(caller) if caller.is_admin() => Ok(()),
        Ok(_) => Err(ApiError::forbidden(
            "metrics of every host and project are for a cloud admin or the scrape token",
        )),
        Err(_) => Err(ApiError::unauthorized("invalid token")),
    }
}

/// Compare without stopping at the first byte that differs.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use pistack_types::{kind, Caller, Id};
    use tower::ServiceExt;

    const SCRAPE_TOKEN: &str = "scrape-token-0123456789";

    fn control(store: &Store) -> Arc<ControlPlane> {
        Arc::new(ControlPlane::new(
            None,
            CommandQueue::default(),
            store.clone(),
            None,
        ))
    }

    fn app(store: &Store, token: Option<&str>, dev: bool) -> (Router, gate::Keys) {
        let keys = gate::Keys::load_or_create(store).unwrap();
        let state = Scrape {
            verifier: keys.verifier(),
            token: scrape_token(token),
            dev,
            gauge: gauge::Client::new(store.clone(), Arc::new(gauge::SampleRing::memory())),
            control: control(store),
        };
        let app = Router::new()
            .route("/metrics", get(scrape))
            .with_state(state);
        (app, keys)
    }

    fn token_for(keys: &gate::Keys, system: bool) -> String {
        let caller = Caller {
            principal: Id::new(kind::PRINCIPAL),
            principal_name: "someone".into(),
            project: Some(Id::new(kind::PROJECT)),
            roles: vec!["admin".into()],
            system,
        };
        keys.sign(&caller, chrono::Duration::minutes(5)).unwrap().0
    }

    async fn get_metrics(app: &Router, bearer: Option<&str>) -> (StatusCode, String) {
        let mut request = Request::builder().uri("/metrics");
        if let Some(token) = bearer {
            request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn metrics_need_the_scrape_token_or_a_cloud_admin() {
        let store = Store::in_memory().unwrap();
        let (app, keys) = app(&store, Some(SCRAPE_TOKEN), false);

        let (status, text) = get_metrics(&app, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!text.contains("pistack_"), "{text}");
        let (status, _) = get_metrics(&app, Some("scrape-token-0123456780")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // The admin of one project is not a cloud admin.
        let (status, text) = get_metrics(&app, Some(&token_for(&keys, false))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(!text.contains("pistack_"), "{text}");

        let (status, text) = get_metrics(&app, Some(SCRAPE_TOKEN)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(text.contains("pistack_build_info{build="), "{text}");
        let (status, _) = get_metrics(&app, Some(&token_for(&keys, true))).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn without_a_scrape_token_only_an_admin_token_works() {
        let store = Store::in_memory().unwrap();
        // Too short to be a secret: not used.
        let (app, keys) = app(&store, Some("short"), false);
        let (status, _) = get_metrics(&app, Some("short")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = get_metrics(&app, Some(&token_for(&keys, true))).await;
        assert_eq!(status, StatusCode::OK);

        // Dev mode asks nobody for anything, here as everywhere.
        let (dev, _) = super::tests::app(&store, None, true);
        let (status, _) = get_metrics(&dev, None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_server_reports_its_queue_its_store_and_its_reconcilers() {
        let store = Store::in_memory().unwrap();
        store.put_meta("something", &1u32).unwrap();
        let queue = CommandQueue::default();
        queue.hello(&pistack_rpc::AgentHello {
            node_name: "h1".into(),
            capabilities: vec![],
            resources: Default::default(),
            version: String::new(),
            labels: Default::default(),
            underlay_ip: None,
        });
        let _ticket = queue
            .enqueue(
                "h1",
                pistack_rpc::CommandKind::StartMachine,
                serde_json::Value::Null,
            )
            .unwrap();
        pistack_types::tally::reconcile_failed("forge");
        let control = ControlPlane::new(None, queue, store.clone(), None);
        control.refresh_tables();
        let samples: std::collections::BTreeMap<String, f64> =
            control.samples().into_iter().collect();

        assert_eq!(samples["pistack_agent_commands_queued{host=\"h1\"}"], 1.0);
        assert_eq!(
            samples["pistack_agent_commands_in_flight{host=\"h1\"}"],
            0.0
        );
        assert!(samples["pistack_reconcile_failures_total{module=\"forge\"}"] >= 1.0);
        // A module with no failure is there too, at zero.
        assert_eq!(
            samples["pistack_reconcile_failures_total{module=\"shelf\"}"],
            0.0
        );
        assert!(samples["pistack_store_table_rows{table=\"meta\"}"] >= 1.0);
        assert!(samples.contains_key("pistack_store_log_entries"));
        // No cluster handle, no cluster series.
        assert!(!samples.keys().any(|k| k.starts_with("pistack_cluster_")));
    }

    #[tokio::test]
    async fn a_leader_reports_its_role_its_log_and_how_far_each_follower_is() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_memory().unwrap();
        let members = vec![
            Member::voter("n1", "http://10.0.0.11:8443"),
            Member::voter("n2", "http://10.0.0.12:8443"),
            Member {
                role: Role::Learner,
                ..Member::voter("n3", "http://10.0.0.13:8443")
            },
        ];
        store.put_meta("members", &members).unwrap();
        store.put_meta("cluster_commit", &120u64).unwrap();
        let established = crate::cluster::Established {
            role: Role::Voter,
            leader: true,
            leader_api: "http://10.0.0.11:8443".into(),
            term: 7,
            members: members.clone(),
        };
        let handle = Arc::new(crate::cluster::handle_from(
            "n1".into(),
            "http://10.0.0.11:8443".into(),
            "c1".into(),
            store.clone(),
            dir.path().join("state.redb"),
            &established,
            None,
            pistack_types::Leadership::new(true),
        ));
        handle
            .progress
            .lock()
            .unwrap()
            .insert("n2".into(), (112, std::time::Instant::now()));
        let control = ControlPlane::new(
            Some(handle.clone()),
            CommandQueue::default(),
            store.clone(),
            None,
        );
        let read = || -> std::collections::BTreeMap<String, f64> {
            control.samples().into_iter().collect()
        };

        let samples = read();
        assert_eq!(samples["pistack_cluster_is_leader{server=\"n1\"}"], 1.0);
        assert_eq!(samples["pistack_cluster_is_voter{server=\"n1\"}"], 1.0);
        assert_eq!(samples["pistack_cluster_term{server=\"n1\"}"], 7.0);
        assert_eq!(
            samples["pistack_cluster_commit_index{server=\"n1\"}"],
            120.0
        );
        assert_eq!(
            samples["pistack_cluster_members{role=\"voter\",server=\"n1\"}"],
            2.0
        );
        assert_eq!(
            samples["pistack_cluster_members{role=\"learner\",server=\"n1\"}"],
            1.0
        );
        let lag = "pistack_cluster_follower_lag_entries{member=\"n2\",server=\"n1\"}";
        assert_eq!(samples[lag], 8.0);
        let age = "pistack_cluster_follower_last_pull_seconds{member=\"n2\",server=\"n1\"}";
        assert!(samples[age] < 5.0);
        // n3 never pulled: nothing is claimed about it.
        assert!(!samples.keys().any(|k| k.contains("member=\"n3\"")));

        // A follower knows its own state and nobody's lag.
        handle.leader.store(false, Ordering::Release);
        let samples = read();
        assert_eq!(samples["pistack_cluster_is_leader{server=\"n1\"}"], 0.0);
        assert!(!samples.contains_key(lag));
    }
}
