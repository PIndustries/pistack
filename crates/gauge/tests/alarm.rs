// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Post a sample, create an alarm, post a crossing sample: the alarm fires.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::{DateTime, Duration, Utc};
use gauge::{
    Alarm, AlarmPhase, AlarmSpec, Client, Comparison, Gauge, Sample, SampleRing, MAX_HOLD,
};
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::module::CancellationToken;
use pistack_types::{kind, Caller, Id, Meta, Module, ModuleContext};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

struct Harness {
    router: axum::Router,
}

fn harness() -> Harness {
    let store = Store::in_memory().unwrap();
    let gauge = Gauge;
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    assert_eq!(gauge.info().name, "gauge");
    assert_eq!(gauge.info().prefix, "gauge");
    Harness {
        router: gauge.router(ctx),
    }
}

impl Harness {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, String) {
        self.call_as(admin_caller(), method, path, body).await
    }

    async fn call_as(
        &self,
        caller: Caller,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, String) {
        let mut req = Request::builder().method(method).uri(path);
        let body = match body {
            Some(v) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let mut req = req.body(body).unwrap();
        req.extensions_mut().insert(caller);
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }
}

fn json_body(status: StatusCode, raw: &str) -> Value {
    if raw.is_empty() {
        return Value::Null;
    }
    serde_json::from_str(raw).unwrap_or_else(|e| panic!("status {status} not json ({e}): {raw}"))
}

#[tokio::test]
async fn crossing_sample_fires_alarm() {
    let h = harness();
    // Count from whatever the ring already holds, so samples left by an
    // earlier run or another test cannot change the result.
    let (st, raw) = h.call("GET", "/samples?metric=cpu.usage", None).await;
    let before = json_body(st, &raw).as_array().map_or(0, Vec::len);

    let (st, raw) = h
        .call(
            "POST",
            "/samples",
            Some(json!({"metric": "cpu.usage", "value": 10.0})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{raw}");

    let (st, raw) = h
        .call(
            "POST",
            "/alarms",
            Some(json!({
                "name": "cpu-high",
                "spec": {
                    "name": "cpu-high",
                    "metric": "cpu.usage",
                    "comparison": "gt",
                    "threshold": 50.0,
                    "for_seconds": 0
                }
            })),
        )
        .await;
    let alarm = json_body(st, &raw);
    assert_eq!(st, StatusCode::CREATED, "{alarm}");
    let id = alarm["id"].as_str().unwrap();
    assert!(id.starts_with("alarm-"), "{id}");
    assert_eq!(alarm["status"]["phase"], "Ok");
    assert_eq!(alarm["status"]["last_value"], 10.0);

    let (st, raw) = h
        .call(
            "POST",
            "/samples",
            Some(json!({"metric": "cpu.usage", "value": 80.0})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{raw}");

    let (st, raw) = h.call("GET", &format!("/alarms/{id}"), None).await;
    let alarm = json_body(st, &raw);
    assert_eq!(st, StatusCode::OK, "{alarm}");
    assert_eq!(alarm["status"]["phase"], "Firing");
    assert_eq!(alarm["status"]["last_value"], 80.0);

    let (st, text) = h.call("GET", "/metrics", None).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(
        text.lines()
            .any(|l| l == "cpu_usage 80" || l == "cpu_usage 80.0"),
        "{text}"
    );

    let (st, raw) = h.call("GET", "/samples?metric=cpu.usage", None).await;
    let samples = json_body(st, &raw);
    assert_eq!(st, StatusCode::OK, "{samples}");
    assert_eq!(samples.as_array().unwrap().len(), before + 2);
}

fn admin_caller() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "dev-admin".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    }
}

#[tokio::test]
async fn missing_caller_is_unauthorized() {
    let h = harness();
    let res = h
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/alarms")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let raw = String::from_utf8(
        axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let body = json_body(status, &raw);
    assert_eq!(body["error"]["code"], "unauthorized");
    assert_eq!(body["error"]["message"], "no credentials");
}

fn member_of(project: &Id) -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "member".into(),
        project: Some(project.clone()),
        roles: vec!["member".into()],
        system: false,
    }
}

#[tokio::test]
async fn pistack_series_are_for_the_admin_and_each_project_sees_its_own() {
    let h = harness();
    let mine = Id::new(kind::PROJECT);
    let theirs = Id::new(kind::PROJECT);
    let memory = |machine: &str, project: &Id| {
        gauge::series(
            gauge::catalog::MACHINE_MEMORY,
            &[("machine", machine), ("project", &project.to_string())],
        )
    };
    gauge::Gauge
        .client(Store::in_memory().unwrap())
        .record_all(vec![
            (memory("scope-mine", &mine), 1.0),
            (memory("scope-theirs", &theirs), 2.0),
            (
                gauge::series(gauge::catalog::HOST_LOAD1, &[("host", "scope-h1")]),
                3.0,
            ),
        ]);
    let member = member_of(&mine);

    // The exposition has every project and host in it: admin only.
    let (st, text) = h.call("GET", "/metrics", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        text.contains("scope-theirs") && text.contains("scope-h1"),
        "{text}"
    );
    let (st, raw) = h.call_as(member.clone(), "GET", "/metrics", None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{raw}");
    assert!(!raw.contains("scope-"), "{raw}");

    // The samples a member lists leave out hosts and other projects.
    let (st, raw) = h.call_as(member.clone(), "GET", "/samples", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(raw.contains("scope-mine"), "{raw}");
    assert!(
        !raw.contains("scope-theirs") && !raw.contains("scope-h1"),
        "{raw}"
    );
    let (_, raw) = h.call("GET", "/samples", None).await;
    assert!(
        raw.contains("scope-theirs") && raw.contains("scope-h1"),
        "{raw}"
    );

    // Nobody posts under PiStack's own names, the admin included.
    for caller in [member, admin_caller()] {
        let body = json!({"metric": "pistack_host_load1{host=\"scope-h1\"}", "value": 99.0});
        let (st, raw) = h.call_as(caller, "POST", "/samples", Some(body)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{raw}");
    }
}

/// A hold is measured on the stamps of the samples themselves. An alarm fires when the
/// newest run of crossing samples spans `for_seconds` between the stamp of its first
/// sample and the stamp of the newest, and the first sample that does not cross ends the
/// run. Nothing is counted in rounds and nothing is read from the clock of the server
/// deciding it, so every test here stamps its own samples and waits on nothing: an alarm
/// on a series reporting every fifteen seconds holds for `60` the way it does on one
/// reporting every minute.
///
/// A fresh store and a fresh window per test: the window the value is read from belongs
/// to the client that filled it.
fn client() -> Client {
    Client::new(Store::in_memory().unwrap(), Arc::new(SampleRing::memory()))
}

fn alarm(c: &Client, name: &str, metric: &str, at: f64, for_seconds: u32) -> Alarm {
    c.create_alarm(
        Meta::new(kind::ALARM, name, None),
        AlarmSpec {
            name: name.into(),
            metric: metric.into(),
            comparison: Comparison::Gt,
            threshold: at,
            for_seconds,
        },
    )
    .unwrap()
}

fn phase(c: &Client, a: &Alarm) -> AlarmPhase {
    c.get_alarm(None, &a.meta.id.to_string())
        .unwrap()
        .status
        .phase
}

/// The origin of these tests' stamps: a series reporting from a fixed moment, so the
/// stamps are the only clock a hold here has.
fn epoch() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

/// Record a sample stamped `at`, and say what phase the alarm was left in.
fn post(c: &Client, a: &Alarm, metric: &str, value: f64, at: DateTime<Utc>) -> AlarmPhase {
    c.record(Sample {
        metric: metric.into(),
        value,
        timestamp: at,
    })
    .unwrap();
    phase(c, a)
}

/// `i` seconds after the origin.
fn at(i: i64) -> DateTime<Utc> {
    epoch() + Duration::seconds(i)
}

/// The revision of an alarm as the store holds it.
fn revision(c: &Client, a: &Alarm) -> u64 {
    c.get_alarm(None, &a.meta.id.to_string())
        .unwrap()
        .meta
        .revision
}

#[tokio::test]
async fn a_hold_of_sixty_seconds_fires_on_the_fifths_sample_of_a_fifteen_second_series() {
    let c = client();
    let metric = "hold.busy";
    let held = alarm(&c, "held", metric, 50.0, 60);
    assert_eq!(
        phase(&c, &held),
        AlarmPhase::Pending,
        "an alarm on a series nobody has reported is decided by nothing"
    );

    // Stamps 0, 15, 30, 45: the value has crossed for forty-five seconds, which is not
    // sixty. The fifth crossing sample, stamped 60, spans the hold and fires.
    let phases = [0i64, 15, 30, 45, 60]
        .into_iter()
        .map(|s| post(&c, &held, metric, 80.0, at(s)))
        .collect::<Vec<_>>();
    assert_eq!(
        phases,
        vec![
            AlarmPhase::Ok,
            AlarmPhase::Ok,
            AlarmPhase::Ok,
            AlarmPhase::Ok,
            AlarmPhase::Firing
        ],
        "a hold of sixty seconds did not wait sixty seconds of stamps"
    );
    assert_eq!(
        c.get_alarm(None, &held.meta.id.to_string())
            .unwrap()
            .status
            .last_value,
        Some(80.0),
        "the alarm fires with the value that decided it"
    );
}

#[tokio::test]
async fn a_hold_is_measured_on_the_stamps_and_not_the_reports_of_the_series() {
    let c = client();
    let metric = "gap.busy";
    let held = alarm(&c, "gap", metric, 50.0, 60);

    // Three samples, six seconds apart, spanning ten seconds: nothing close to a minute.
    assert_eq!(post(&c, &held, metric, 80.0, at(0)), AlarmPhase::Ok);
    assert_eq!(post(&c, &held, metric, 80.0, at(3)), AlarmPhase::Ok);
    assert_eq!(post(&c, &held, metric, 80.0, at(10)), AlarmPhase::Ok);

    // A gap of fifty seconds between reports, then a crossing stamped on the far side of
    // it. The run spans sixty seconds on the series' own clock, and that is the wait the
    // alarm asked for — a series that stops reporting for a minute has been crossing for
    // a minute, as far as its stamps can say.
    assert_eq!(post(&c, &held, metric, 80.0, at(60)), AlarmPhase::Firing);

    // Irregular stamps the other way: two samples a quarter of a second apart cannot
    // stand in for the minute the run still needs.
    let tight = alarm(&c, "tight", metric, 50.0, 60);
    assert_eq!(
        phase(&c, &tight),
        AlarmPhase::Firing,
        "the window is the window"
    );
    let _ = post(&c, &tight, metric, 80.0, at(90));
    assert_eq!(
        post(
            &c,
            &tight,
            metric,
            90.0,
            at(90) + Duration::milliseconds(250)
        ),
        AlarmPhase::Firing,
        "a stamp of the same moment extends a run by nothing, and a quarter of a second \
         by less still"
    );
}

#[tokio::test]
async fn a_hold_fires_the_moment_a_sample_before_it_is_made_spans_it() {
    let c = client();
    let metric = "old.busy";
    // The value has been crossing for two minutes, on stamps the window holds.
    for s in [0i64, 30, 60, 90, 120] {
        c.record(Sample {
            metric: metric.into(),
            value: 80.0,
            timestamp: at(s),
        })
        .unwrap();
    }
    // A hold is a fact about the stamps and not about how long the alarm has been
    // watching them: the alarm is decided from the samples there when it is made.
    let held = alarm(&c, "old", metric, 50.0, 60);
    assert_eq!(phase(&c, &held), AlarmPhase::Firing);

    // A run only as long as the stamps say it is, and the same series asked for more.
    let thin = alarm(&c, "thin", "short.run", 50.0, 60);
    assert_eq!(phase(&c, &thin), AlarmPhase::Pending);
    assert_eq!(post(&c, &thin, "short.run", 80.0, at(200)), AlarmPhase::Ok);
}

#[tokio::test]
async fn one_sample_under_the_threshold_ends_the_run() {
    let c = client();
    let metric = "dip.busy";
    let held = alarm(&c, "dip", metric, 50.0, 30);

    // Crossing for a minute, so a hold of thirty seconds is spanned and it fires.
    for s in [0i64, 15, 30, 45, 60] {
        post(&c, &held, metric, 84.0, at(s));
    }
    assert_eq!(phase(&c, &held), AlarmPhase::Firing);

    // The first sample that does not cross clears it, whatever came before.
    assert_eq!(post(&c, &held, metric, 20.0, at(75)), AlarmPhase::Ok);

    // Crossing again, and the wait runs all over again on stamps: the minute before the
    // dip was a run that ended, and the run after it spans nothing until its own stamps
    // say it spans thirty seconds. The stamp of the sample is what a run is measured to,
    // so the run fires on the sample stamped thirty seconds after the one that began it.
    assert_eq!(post(&c, &held, metric, 90.0, at(90)), AlarmPhase::Ok);
    assert_eq!(post(&c, &held, metric, 90.0, at(104)), AlarmPhase::Ok);
    assert_eq!(post(&c, &held, metric, 90.0, at(119)), AlarmPhase::Ok);
    assert_eq!(
        post(&c, &held, metric, 90.0, at(120)),
        AlarmPhase::Firing,
        "a hold counted the run before the sample that ended it, or fired off a run that          spanned less than it asked for"
    );
}

#[tokio::test]
async fn an_alarm_with_no_for_seconds_is_decided_from_one_sample() {
    let c = client();
    let metric = "now.busy";
    // Stamps near the moment the test runs: an alarm with no hold is decided from the
    // latest sample of a series that is still reporting, and a series whose newest point
    // is older than `STALE_AFTER` is a machine that stopped reporting.
    let stamp = Utc::now();
    c.record(Sample {
        metric: metric.into(),
        value: 10.0,
        timestamp: stamp,
    })
    .unwrap();
    let now = alarm(&c, "now", metric, 50.0, 0);
    assert_eq!(
        phase(&c, &now),
        AlarmPhase::Ok,
        "the sample there when it was made decides an alarm with no hold"
    );
    assert_eq!(
        post(&c, &now, metric, 80.0, stamp + Duration::seconds(1)),
        AlarmPhase::Firing
    );
    assert_eq!(
        post(&c, &now, metric, 10.0, stamp + Duration::seconds(2)),
        AlarmPhase::Ok
    );

    // The first crossing sample fires an alarm with no hold at once, whatever moment it
    // reports for.
    let first = alarm(&c, "first", "first.busy", 50.0, 0);
    assert_eq!(
        post(
            &c,
            &first,
            "first.busy",
            80.0,
            stamp + Duration::seconds(500)
        ),
        AlarmPhase::Firing
    );

    // An alarm asks for a hold by name, so leaving `for_seconds` out asks for none, and
    // the alarm is decided from the sample there when it is made.
    let h = harness();
    let (st, raw) = h
        .call(
            "POST",
            "/samples",
            Some(json!({"metric": "now.busy", "value": 5.0})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{raw}");
    let (st, raw) = h
        .call(
            "POST",
            "/alarms",
            Some(json!({
                "name": "no-for",
                "spec": {
                    "name": "no-for",
                    "metric": "now.busy",
                    "comparison": "gt",
                    "threshold": 10.0
                }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{raw}");
    assert_eq!(json_body(st, &raw)["status"]["phase"], "Ok");
    assert_eq!(
        json_body(st, &raw)["spec"]["for_seconds"],
        0,
        "an alarm that does not ask for a hold is given none"
    );
}

#[tokio::test]
async fn an_update_is_decided_from_the_stamps_and_not_from_the_run_before_it() {
    let c = client();
    let metric = "edit.busy";
    let held = alarm(&c, "edit", metric, 50.0, 30);
    for s in [0i64, 20, 40] {
        post(&c, &held, metric, 80.0, at(s));
    }
    assert_eq!(
        phase(&c, &held),
        AlarmPhase::Firing,
        "forty seconds of crossing spans a hold of thirty"
    );

    // The same alarm, its threshold moved. The window is what it was, and the new alarm
    // is decided from it: nothing was counted for the old one, so nothing ends.
    let before = c.get_alarm(None, &held.meta.id.to_string()).unwrap();
    let mut edited = before.clone();
    edited.spec.threshold = 500.0;
    let edited = c.update_alarm(edited, Some(before.meta.revision)).unwrap();
    assert!(edited.meta.revision > before.meta.revision);

    // A sample the new threshold does not cross, and the phase follows on the next
    // decision: what the old threshold was crossed for is nothing the new one sees.
    c.record(Sample {
        metric: metric.into(),
        value: 80.0,
        timestamp: at(60),
    })
    .unwrap();
    assert_eq!(
        phase(&c, &edited),
        AlarmPhase::Ok,
        "an updated alarm kept a phase it was never decided to"
    );

    // And the hold it now asks for is spanned by the stamps of the crossings the new
    // threshold agrees with.
    for s in [80i64, 100, 120] {
        c.record(Sample {
            metric: metric.into(),
            value: 800.0,
            timestamp: at(s),
        })
        .unwrap();
    }
    assert_eq!(
        phase(&c, &edited),
        AlarmPhase::Firing,
        "an updated alarm never fired off its own threshold"
    );

    // A deleted alarm leaves nothing behind: the window is the only state a hold has.
    c.delete_alarm(&edited.meta.id).unwrap();
    assert!(c.list_alarms(None).unwrap().is_empty());
    c.record(Sample {
        metric: metric.into(),
        value: 900.0,
        timestamp: at(140),
    })
    .unwrap();
}

#[tokio::test]
async fn a_firing_alarm_clears_when_a_sample_stops_crossing() {
    let c = client();
    let metric = "clear.busy";
    let held = alarm(&c, "clear", metric, 50.0, 2);
    assert_eq!(post(&c, &held, metric, 85.0, at(0)), AlarmPhase::Ok);
    assert_eq!(post(&c, &held, metric, 85.0, at(2)), AlarmPhase::Firing);
    // The first sample that does not cross clears it, and the crossing after it is a run
    // of one stamp until its own stamps span the hold again.
    assert_eq!(post(&c, &held, metric, 5.0, at(4)), AlarmPhase::Ok);
    assert_eq!(post(&c, &held, metric, 85.0, at(5)), AlarmPhase::Ok);
    assert_eq!(post(&c, &held, metric, 85.0, at(6)), AlarmPhase::Ok);
    assert_eq!(post(&c, &held, metric, 85.0, at(7)), AlarmPhase::Firing);
}

#[tokio::test]
async fn a_hold_survives_the_writes_of_the_samples_it_is_waited_out_on() {
    let c = client();
    let metric = "quiet.busy";
    // A hold of a minute and a half, waited out by a series reporting every fifty-five
    // seconds: no two reports are ninety seconds apart, so only the run of them spans the
    // hold.
    let held = alarm(&c, "quiet", metric, 50.0, 90);
    // A sample keeping a hold leaves the phase unchanged, so the only thing one can write
    // is how long the wait has got to. Each report here lengthens the run, so each is a
    // write and each write moves the alarm's revision. That move is the server's own
    // bookkeeping: it changes nothing the alarm asks for, and a hold measured on the
    // stamps of samples is not begun again by it.
    let mut phases = Vec::new();
    let mut revisions = Vec::new();
    for round in 0..8i64 {
        // The value turns over every report, so each leaves one the status has never
        // held — which is what a busy series is between two refreshes.
        phases.push(post(&c, &held, metric, 70.0 + round as f64, at(55 * round)));
        revisions.push(revision(&c, &held));
    }
    let wrote = revisions.windows(2).filter(|w| w[1] > w[0]).count();
    assert!(
        wrote >= 2,
        "the hold was waited out on samples that wrote nothing, so this says nothing about \
         a hold surviving a write: {revisions:?}"
    );
    // Fifty-five, a hundred and ten, a hundred and sixty-five: the run spans ninety
    // seconds on the third report of it, and the writes of the samples before neither cut
    // the wait short nor began it again.
    assert!(
        !phases[..2].contains(&AlarmPhase::Firing),
        "the alarm fired before its stamps spanned the hold: {phases:?}"
    );
    assert_eq!(
        phases[2..].iter().position(|p| *p == AlarmPhase::Firing),
        Some(0),
        "a hold of ninety seconds fired somewhere else: {phases:?} at {revisions:?}"
    );
}

#[tokio::test]
async fn a_hold_longer_than_the_window_can_cover_is_refused() {
    let c = client();
    // The window keeps 240 points of one series for an hour, so the longest run its
    // stamps can ever span is an hour. A hold asking for more could never be spanned.
    let too_long = MAX_HOLD.num_seconds() as u32 + 1;
    let refused = c.create_alarm(
        Meta::new(kind::ALARM, "forever", None),
        AlarmSpec {
            name: "forever".into(),
            metric: "any.series".into(),
            comparison: Comparison::Gt,
            threshold: 50.0,
            for_seconds: too_long,
        },
    );
    let err = match refused {
        Ok(_) => panic!("a hold the window cannot cover was accepted"),
        Err(e) => e,
    };
    let message = err.message.clone();
    assert_eq!(err.code, ErrorCode::Unprocessable, "{err}");
    assert!(
        message.contains(&MAX_HOLD.num_seconds().to_string()),
        "the refusal says how long the longest hold possible is: {message}"
    );
    assert!(
        message.contains("for_seconds") && message.contains("window"),
        "the refusal does not say what it refused: {message}"
    );

    // The longest hold that can be covered is accepted.
    let longest = alarm(
        &c,
        "hour",
        "any.series",
        50.0,
        MAX_HOLD.num_seconds() as u32,
    );
    assert_eq!(phase(&c, &longest), AlarmPhase::Pending);

    // And the same refusal on an update to an alarm already made.
    let mut edited = c
        .get_alarm(None, &longest.meta.id.to_string())
        .unwrap()
        .clone();
    edited.spec.for_seconds = too_long;
    let err = c
        .update_alarm(edited, None)
        .expect_err("an update to an impossible hold was accepted");
    assert_eq!(err.code, ErrorCode::Unprocessable, "{err}");
    assert!(
        err.message.contains(&MAX_HOLD.num_seconds().to_string()),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn a_hold_measured_on_stamps_says_the_same_from_the_api_it_is_written_through() {
    let h = harness();
    let (st, raw) = h
        .call(
            "POST",
            "/alarms",
            Some(json!({
                "name": "too-long",
                "spec": {
                    "name": "too-long",
                    "metric": "api.busy",
                    "comparison": "gt",
                    "threshold": 50.0,
                    "for_seconds": MAX_HOLD.num_seconds() + 1
                }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{raw}");
    assert!(
        raw.contains(&MAX_HOLD.num_seconds().to_string()),
        "the refusal the API gives names the longest hold possible: {raw}"
    );
}
