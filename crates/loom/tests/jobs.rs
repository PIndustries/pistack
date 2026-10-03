// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Job and CronJob controllers driven pass by pass, with pod outcomes
//! written by the test in place of the pod controller.

use chrono::{Duration, Utc};
use loom::controllers::{cronjob::CronJobController, job::JobController, Controller};
use loom::{
    Container, CronJob, CronJobSpec, Job, JobSpec, Pod, PodPhase, PodSpec, PodTemplate,
    RestartPolicy, OWNER_LABEL,
};
use pistack_store::Store;
use pistack_types::Resource;

fn template() -> PodTemplate {
    PodTemplate {
        labels: [("job".to_string(), "batch".to_string())].into(),
        annotations: Default::default(),
        spec: PodSpec {
            containers: vec![Container {
                name: "work".into(),
                image: "busybox".into(),
                ..Default::default()
            }],
            restart_policy: RestartPolicy::Never,
            ..Default::default()
        },
    }
}

fn finish(store: &Store, pod: &Pod, phase: PodPhase) {
    store
        .collection::<Pod>()
        .modify(&pod.meta.id, |p| {
            p.status.phase = phase;
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn job_runs_to_completion_with_parallelism() {
    let store = Store::in_memory().unwrap();
    let jobs = store.collection::<Job>();
    let pods = store.collection::<Pod>();
    let ctl = JobController::new(store.clone());

    let job = jobs
        .create(Resource::new(
            "batch",
            None,
            JobSpec {
                template: template(),
                completions: 3,
                parallelism: 2,
                backoff_limit: 6,
                active_deadline_seconds: None,
                ttl_seconds_after_finished: None,
                suspend: false,
            },
        ))
        .unwrap();

    ctl.reconcile_once().await.unwrap();
    let running = pods.list(None).unwrap();
    assert_eq!(running.len(), 2, "parallelism caps active pods");
    assert!(running
        .iter()
        .all(|p| p.meta.labels.get(OWNER_LABEL) == Some(&job.meta.id.to_string())));
    assert!(running.iter().all(|p| p.meta.name.starts_with("batch-")));
    let j = jobs.get(&job.meta.id).unwrap();
    assert_eq!(j.status.active, 2);
    assert!(j.status.start_time.is_some());

    // Two succeed: one more is needed, so exactly one new pod appears.
    for p in &running {
        finish(&store, p, PodPhase::Succeeded);
    }
    ctl.reconcile_once().await.unwrap();
    let all = pods.list(None).unwrap();
    assert_eq!(all.len(), 3);
    let j = jobs.get(&job.meta.id).unwrap();
    assert_eq!((j.status.active, j.status.succeeded), (1, 2));

    let last = all
        .iter()
        .find(|p| p.status.phase == PodPhase::Pending)
        .unwrap();
    finish(&store, last, PodPhase::Succeeded);
    ctl.reconcile_once().await.unwrap();
    let j = jobs.get(&job.meta.id).unwrap();
    assert!(j.status.has_condition("Complete"));
    assert_eq!(j.status.succeeded, 3);
    assert!(j.status.completion_time.is_some());
    // Idempotent once finished.
    ctl.reconcile_once().await.unwrap();
    assert_eq!(pods.list(None).unwrap().len(), 3);
}

#[tokio::test]
async fn job_backs_off_and_fails_past_the_limit() {
    let store = Store::in_memory().unwrap();
    let jobs = store.collection::<Job>();
    let pods = store.collection::<Pod>();
    let ctl = JobController::new(store.clone());

    let job = jobs
        .create(Resource::new(
            "flaky",
            None,
            JobSpec {
                template: template(),
                completions: 1,
                parallelism: 1,
                backoff_limit: 1,
                active_deadline_seconds: None,
                ttl_seconds_after_finished: None,
                suspend: false,
            },
        ))
        .unwrap();
    ctl.reconcile_once().await.unwrap();
    let first = pods.list(None).unwrap().remove(0);
    finish(&store, &first, PodPhase::Failed);

    // Backoff: no replacement yet, next_retry recorded.
    ctl.reconcile_once().await.unwrap();
    assert_eq!(pods.list(None).unwrap().len(), 1);
    let j = jobs.get(&job.meta.id).unwrap();
    assert_eq!(j.status.failed, 1);
    assert!(j.status.next_retry.is_some());
    assert!(!j.status.is_finished());

    // Pretend the failure was long ago (the controller dates a failure by
    // the pod's Ready condition): the retry pod is created.
    pods.modify(&first.meta.id, |p| {
        loom::set_condition(&mut p.status.conditions, "Ready", false, "Failed", "");
        for c in &mut p.status.conditions {
            c.last_transition = Utc::now() - Duration::minutes(10);
        }
        Ok(())
    })
    .unwrap();
    ctl.reconcile_once().await.unwrap();
    let all = pods.list(None).unwrap();
    assert_eq!(all.len(), 2, "retry pod created after backoff");
    let second = all
        .iter()
        .find(|p| p.status.phase == PodPhase::Pending)
        .unwrap()
        .clone();
    finish(&store, &second, PodPhase::Failed);
    ctl.reconcile_once().await.unwrap();
    let j = jobs.get(&job.meta.id).unwrap();
    assert!(
        j.status.has_condition("Failed"),
        "{:?}",
        j.status.conditions
    );
    assert_eq!(j.status.failed, 2);
}

#[tokio::test]
async fn cronjob_fires_and_prunes_history() {
    let store = Store::in_memory().unwrap();
    let crons = store.collection::<CronJob>();
    let jobs = store.collection::<Job>();
    let ctl = CronJobController::new(store.clone());

    let mut cron: CronJob = Resource::new(
        "tick",
        None,
        CronJobSpec {
            schedule: "* * * * *".into(),
            job_template: JobSpec {
                template: template(),
                completions: 1,
                parallelism: 1,
                backoff_limit: 0,
                active_deadline_seconds: None,
                ttl_seconds_after_finished: None,
                suspend: false,
            },
            suspend: false,
            concurrency_policy: loom::ConcurrencyPolicy::Forbid,
            starting_deadline_seconds: None,
            successful_jobs_history_limit: 1,
            failed_jobs_history_limit: 1,
        },
    );
    // Created three minutes ago: a firing is due.
    cron.meta.created_at = Utc::now() - Duration::minutes(3);
    let cron = crons.create(cron).unwrap();

    ctl.reconcile_once().await.unwrap();
    let js = jobs.list(None).unwrap();
    assert_eq!(js.len(), 1, "one job for the most recent missed slot");
    assert!(js[0].meta.name.starts_with("tick-"));
    assert_eq!(
        js[0].meta.labels.get(OWNER_LABEL).unwrap(),
        &cron.meta.id.to_string()
    );
    let c = crons.get(&cron.meta.id).unwrap();
    assert_eq!(c.status.active, vec![js[0].meta.id.clone()]);
    assert!(c.status.last_schedule_time.is_some());
    let next = c.status.next_schedule_time.unwrap();
    assert!(next > Utc::now() - Duration::minutes(1));

    // Forbid: a due slot with the job still active is skipped.
    crons
        .modify(&cron.meta.id, |c| {
            c.status.next_schedule_time = Some(Utc::now() - Duration::seconds(30));
            Ok(())
        })
        .unwrap();
    ctl.reconcile_once().await.unwrap();
    assert_eq!(jobs.list(None).unwrap().len(), 1);
    let c = crons.get(&cron.meta.id).unwrap();
    assert!(c
        .status
        .conditions
        .iter()
        .any(|k| k.kind == "Skipped" && k.status));

    // Finish the job; force another slot: a second job appears and the
    // history limit of one prunes the finished one. (Jobs are named by
    // their scheduled minute, so free the name the next slot will take.)
    jobs.modify(&js[0].meta.id, |j| {
        j.meta.name = "tick-earlier".into();
        loom::set_condition(&mut j.status.conditions, "Complete", true, "Done", "");
        j.status.completion_time = Some(Utc::now());
        Ok(())
    })
    .unwrap();
    crons
        .modify(&cron.meta.id, |c| {
            c.status.next_schedule_time = Some(Utc::now() - Duration::seconds(30));
            Ok(())
        })
        .unwrap();
    ctl.reconcile_once().await.unwrap();
    let js2 = jobs.list(None).unwrap();
    assert_eq!(js2.len(), 2);
    assert!(!loom::delete_requested(
        js2.iter().find(|j| !j.status.is_finished()).unwrap()
    ));
    // One finished job is within the limit of one, so nothing pruned yet.
    assert!(js2.iter().all(|j| !loom::delete_requested(j)));

    // Finish the second too: now two finished > limit 1, oldest pruned.
    let newest = js2.iter().find(|j| !j.status.is_finished()).unwrap();
    jobs.modify(&newest.meta.id, |j| {
        loom::set_condition(&mut j.status.conditions, "Complete", true, "Done", "");
        j.status.completion_time = Some(Utc::now());
        Ok(())
    })
    .unwrap();
    ctl.reconcile_once().await.unwrap();
    let js3 = jobs.list(None).unwrap();
    let pruned: Vec<_> = js3.iter().filter(|j| loom::delete_requested(j)).collect();
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].meta.id, js[0].meta.id, "oldest goes first");
    let c = crons.get(&cron.meta.id).unwrap();
    assert!(c.status.active.is_empty());
    assert!(c.status.last_successful_time.is_some());
}
