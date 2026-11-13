// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The Job controller: run pods to completion.
//!
//! Keeps up to `parallelism` pods active until `completions` have
//! `Succeeded`. A `Failed` pod counts against `backoff_limit` and delays
//! the next replacement by `10s * 2^failures` (capped at six minutes).
//! Finished jobs get a `Complete` or `Failed` condition and, with
//! `ttl_seconds_after_finished`, are deleted along with their pods.

use super::{
    delete_quietly, owned_by, patch_status, pod_from_template, request_delete, Controller,
};
use crate::types::{delete_requested, set_condition, Job, Pod, PodPhase};
use chrono::{DateTime, Duration, Utc};
use pistack_store::{Collection, Store};

/// Base backoff after a failed pod.
pub const BACKOFF_BASE_SECS: i64 = 10;
/// Cap on the backoff.
pub const BACKOFF_MAX_SECS: i64 = 360;

/// `10s * 2^(failures-1)`, capped.
pub fn backoff_after(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::zero();
    }
    let secs = (BACKOFF_BASE_SECS << (failures - 1).min(10)).min(BACKOFF_MAX_SECS);
    Duration::seconds(secs)
}

pub struct JobController {
    store: Store,
}

impl JobController {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
    fn jobs(&self) -> Collection<Job> {
        self.store.collection::<Job>()
    }
    fn pods(&self) -> Collection<Pod> {
        self.store.collection::<Pod>()
    }

    fn teardown(&self, job: &Job, pods: &[Pod]) -> anyhow::Result<()> {
        let mut remaining = 0;
        for p in owned_by(pods, &job.meta.id) {
            remaining += 1;
            if !delete_requested(p) {
                request_delete(&self.pods(), &p.meta.id)?;
            }
        }
        if remaining == 0 {
            delete_quietly(&self.jobs(), &job.meta.id)?;
            tracing::info!(job = %job.meta.id, name = %job.meta.name, "loom: job deleted");
        }
        Ok(())
    }

    fn converge(&self, job: &Job, pods: &[Pod], now: DateTime<Utc>) -> anyhow::Result<()> {
        let owned: Vec<&Pod> = owned_by(pods, &job.meta.id)
            .filter(|p| !delete_requested(p))
            .collect();
        let succeeded = owned
            .iter()
            .filter(|p| p.status.phase == PodPhase::Succeeded)
            .count() as u32;
        let failed = owned
            .iter()
            .filter(|p| p.status.phase == PodPhase::Failed)
            .count() as u32;
        let active_pods: Vec<&Pod> = owned
            .iter()
            .copied()
            .filter(|p| !p.status.phase.is_terminal())
            .collect();
        let active = active_pods.len() as u32;

        if job.status.is_finished() {
            // TTL: delete the job once it has been finished long enough.
            if let (Some(ttl), Some(done)) = (
                job.spec.ttl_seconds_after_finished,
                job.status.completion_time,
            ) {
                if now - done >= Duration::seconds(ttl as i64) {
                    request_delete(&self.jobs(), &job.meta.id)?;
                }
            }
            return Ok(());
        }

        let start = job.status.start_time.unwrap_or(now);
        let deadline_hit = job
            .spec
            .active_deadline_seconds
            .is_some_and(|d| now - start >= Duration::seconds(d as i64));
        let complete = succeeded >= job.spec.completions;
        let exhausted = failed > job.spec.backoff_limit;

        if complete || exhausted || deadline_hit {
            // Stop whatever is still running.
            for p in &active_pods {
                request_delete(&self.pods(), &p.meta.id)?;
            }
            let reason = if complete {
                ("Complete", "AllPodsSucceeded", String::new())
            } else if exhausted {
                (
                    "Failed",
                    "BackoffLimitExceeded",
                    format!("{failed} pods failed (limit {})", job.spec.backoff_limit),
                )
            } else {
                (
                    "Failed",
                    "DeadlineExceeded",
                    format!(
                        "job ran longer than {}s",
                        job.spec.active_deadline_seconds.unwrap_or(0)
                    ),
                )
            };
            patch_status(&self.jobs(), &job.meta.id, |s| {
                s.active = 0;
                s.succeeded = succeeded;
                s.failed = failed;
                s.start_time = Some(start);
                s.completion_time = Some(now);
                s.next_retry = None;
                set_condition(&mut s.conditions, reason.0, true, reason.1, &reason.2);
            })?;
            tracing::info!(job = %job.meta.id, name = %job.meta.name, result = reason.0, "loom: job finished");
            return Ok(());
        }

        // Backoff: the newest failure sets the earliest next start.
        let last_failure = owned
            .iter()
            .filter(|p| p.status.phase == PodPhase::Failed)
            .filter_map(|p| {
                p.status
                    .conditions
                    .iter()
                    .find(|c| c.kind == "Ready")
                    .map(|c| c.last_transition)
                    .or(Some(p.meta.updated_at))
            })
            .max();
        let next_retry = last_failure.map(|t| t + backoff_after(failed));
        let waiting = next_retry.is_some_and(|t| now < t);

        let mut created = 0u32;
        if !job.spec.suspend && !waiting {
            let want_active = job
                .spec
                .parallelism
                .min(job.spec.completions.saturating_sub(succeeded));
            for _ in active..want_active {
                let pod = pod_from_template(
                    &job.meta.name,
                    job.meta.project.clone(),
                    &job.spec.template,
                    &job.meta.id,
                );
                let pod = self.pods().create(pod)?;
                created += 1;
                tracing::info!(job = %job.meta.id, pod = %pod.meta.id, "loom: job pod created");
            }
        }

        patch_status(&self.jobs(), &job.meta.id, |s| {
            s.active = active + created;
            s.succeeded = succeeded;
            s.failed = failed;
            s.start_time = Some(start);
            s.next_retry = if waiting { next_retry } else { None };
            let (reason, msg) = if job.spec.suspend {
                ("Suspended", String::new())
            } else if waiting {
                (
                    "BackOff",
                    format!(
                        "{failed} failed; retrying at {}",
                        next_retry.map(|t| t.to_rfc3339()).unwrap_or_default()
                    ),
                )
            } else {
                (
                    "Running",
                    format!("{succeeded}/{} succeeded", job.spec.completions),
                )
            };
            set_condition(
                &mut s.conditions,
                "Progressing",
                !job.spec.suspend,
                reason,
                &msg,
            );
        })?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Controller for JobController {
    fn name(&self) -> &'static str {
        "job"
    }
    fn watches(&self) -> &'static [&'static str] {
        &[crate::types::kinds::POD, crate::types::kinds::JOB]
    }
    async fn reconcile_once(&self) -> anyhow::Result<()> {
        let jobs = self.jobs().list(None)?;
        let pods = self.pods().list(None)?;
        let now = Utc::now();
        for job in &jobs {
            let res = if delete_requested(job) {
                self.teardown(job, &pods)
            } else {
                self.converge(job, &pods, now)
            };
            if let Err(e) = res {
                pistack_types::tally::reconcile_failed("loom");
                tracing::warn!(job = %job.meta.id, error = %e, "loom: job reconcile failed");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff_after(0), Duration::zero());
        assert_eq!(backoff_after(1), Duration::seconds(10));
        assert_eq!(backoff_after(2), Duration::seconds(20));
        assert_eq!(backoff_after(3), Duration::seconds(40));
        assert_eq!(backoff_after(6), Duration::seconds(320));
        assert_eq!(backoff_after(7), Duration::seconds(360));
        assert_eq!(backoff_after(60), Duration::seconds(360));
    }
}
