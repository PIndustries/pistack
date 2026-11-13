// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The CronJob controller: create a Job on every schedule firing.
//!
//! Jobs are owned through `OWNER_LABEL` and named `<cron>-<unix minute>`.
//! `concurrency_policy` decides what happens when the previous job is still
//! running; `starting_deadline_seconds` skips a firing that was missed by
//! more than that; history limits prune finished jobs oldest first.

use super::{delete_quietly, owned_by, patch_status, request_delete, Controller};
use crate::cron::Schedule;
use crate::types::{
    delete_requested, set_condition, ConcurrencyPolicy, CronJob, Job, JobStatus, OWNER_LABEL,
};
use chrono::{DateTime, Utc};
use pistack_store::{Collection, Store, StoreError};
use pistack_types::{Id, Resource};

pub struct CronJobController {
    store: Store,
}

impl CronJobController {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
    fn crons(&self) -> Collection<CronJob> {
        self.store.collection::<CronJob>()
    }
    fn jobs(&self) -> Collection<Job> {
        self.store.collection::<Job>()
    }

    fn teardown(&self, cron: &CronJob, jobs: &[Job]) -> anyhow::Result<()> {
        let mut remaining = 0;
        for j in owned_by(jobs, &cron.meta.id) {
            remaining += 1;
            if !delete_requested(j) {
                request_delete(&self.jobs(), &j.meta.id)?;
            }
        }
        if remaining == 0 {
            delete_quietly(&self.crons(), &cron.meta.id)?;
            tracing::info!(cron = %cron.meta.id, name = %cron.meta.name, "loom: cronjob deleted");
        }
        Ok(())
    }

    fn create_job(&self, cron: &CronJob, at: DateTime<Utc>) -> Result<Option<Job>, StoreError> {
        let name = format!("{}-{}", cron.meta.name, at.timestamp() / 60);
        let mut job: Job = Resource::new(
            name,
            cron.meta.project.clone(),
            cron.spec.job_template.clone(),
        );
        job.meta.labels = cron.spec.job_template.template.labels.clone();
        job.meta
            .labels
            .insert(OWNER_LABEL.into(), cron.meta.id.to_string());
        job.meta
            .annotations
            .insert("loom.pistack.io/scheduled-at".into(), at.to_rfc3339());
        job.status = JobStatus::default();
        match self.jobs().create(job) {
            Ok(j) => Ok(Some(j)),
            // Same minute already fired (restart mid-minute): not an error.
            Err(StoreError::Conflict(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn converge(&self, cron: &CronJob, jobs: &[Job], now: DateTime<Utc>) -> anyhow::Result<()> {
        let schedule = match Schedule::parse(&cron.spec.schedule) {
            Ok(s) => s,
            Err(e) => {
                let msg = e.to_string();
                patch_status(&self.crons(), &cron.meta.id, |s| {
                    set_condition(&mut s.conditions, "Valid", false, "BadSchedule", &msg);
                })?;
                return Ok(());
            }
        };
        let owned: Vec<&Job> = owned_by(jobs, &cron.meta.id)
            .filter(|j| !delete_requested(j))
            .collect();
        let active: Vec<Id> = owned
            .iter()
            .filter(|j| !j.status.is_finished())
            .map(|j| j.meta.id.clone())
            .collect();

        // When is the next firing? Anchor on the last one we did, else creation.
        let anchor = cron
            .status
            .last_schedule_time
            .unwrap_or(cron.meta.created_at);
        let mut next = match cron.status.next_schedule_time {
            Some(t) => t,
            None => match schedule.next_after(anchor) {
                Some(t) => t,
                None => {
                    patch_status(&self.crons(), &cron.meta.id, |s| {
                        set_condition(
                            &mut s.conditions,
                            "Valid",
                            false,
                            "NeverFires",
                            "schedule never fires",
                        );
                    })?;
                    return Ok(());
                }
            },
        };

        let mut fired: Option<DateTime<Utc>> = None;
        let mut skipped_reason = String::new();
        if now >= next {
            // Catch up: a long outage fires only the most recent slot.
            let mut latest = next;
            while let Some(t) = schedule.next_after(latest) {
                if t > now {
                    break;
                }
                latest = t;
            }
            let too_late = cron
                .spec
                .starting_deadline_seconds
                .is_some_and(|d| (now - latest).num_seconds() > d as i64);
            if cron.spec.suspend {
                skipped_reason = "suspended".into();
            } else if too_late {
                skipped_reason = "missed starting deadline".into();
            } else {
                let blocked = match cron.spec.concurrency_policy {
                    ConcurrencyPolicy::Allow => false,
                    ConcurrencyPolicy::Forbid => !active.is_empty(),
                    ConcurrencyPolicy::Replace => {
                        for id in &active {
                            request_delete(&self.jobs(), id)?;
                        }
                        false
                    }
                };
                if blocked {
                    skipped_reason = "previous job still active".into();
                } else if let Some(j) = self.create_job(cron, latest)? {
                    tracing::info!(cron = %cron.meta.id, job = %j.meta.id, at = %latest, "loom: cronjob fired");
                    fired = Some(latest);
                }
            }
            if fired.is_none() {
                fired = Some(latest); // mark the slot handled even when skipped
            }
            next = schedule.next_after(latest).unwrap_or(latest);
        }

        // History.
        let mut finished: Vec<&Job> = owned
            .iter()
            .copied()
            .filter(|j| j.status.is_finished())
            .collect();
        finished.sort_by_key(|j| j.meta.created_at);
        let ok: Vec<&Job> = finished
            .iter()
            .copied()
            .filter(|j| j.status.has_condition("Complete"))
            .collect();
        let bad: Vec<&Job> = finished
            .iter()
            .copied()
            .filter(|j| !j.status.has_condition("Complete"))
            .collect();
        for j in ok.iter().take(
            ok.len()
                .saturating_sub(cron.spec.successful_jobs_history_limit as usize),
        ) {
            request_delete(&self.jobs(), &j.meta.id)?;
        }
        for j in bad.iter().take(
            bad.len()
                .saturating_sub(cron.spec.failed_jobs_history_limit as usize),
        ) {
            request_delete(&self.jobs(), &j.meta.id)?;
        }
        let last_ok = ok.iter().filter_map(|j| j.status.completion_time).max();

        let active_after: Vec<Id> = if fired.is_some() {
            // Re-read so the status reflects the job created this pass.
            owned_by(
                &self.jobs().list(cron.meta.project.as_ref())?,
                &cron.meta.id,
            )
            .filter(|j| !delete_requested(j) && !j.status.is_finished())
            .map(|j| j.meta.id.clone())
            .collect()
        } else {
            active
        };
        patch_status(&self.crons(), &cron.meta.id, |s| {
            s.active = active_after.clone();
            if let Some(t) = fired {
                s.last_schedule_time = Some(t);
            }
            s.next_schedule_time = Some(next);
            if last_ok.is_some() {
                s.last_successful_time = last_ok;
            }
            set_condition(
                &mut s.conditions,
                "Valid",
                true,
                "Scheduled",
                &format!("next at {}", next.to_rfc3339()),
            );
            if !skipped_reason.is_empty() {
                set_condition(
                    &mut s.conditions,
                    "Skipped",
                    true,
                    "LastRunSkipped",
                    &skipped_reason,
                );
            } else if fired.is_some() {
                set_condition(&mut s.conditions, "Skipped", false, "Fired", "");
            }
        })?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Controller for CronJobController {
    fn name(&self) -> &'static str {
        "cronjob"
    }
    fn watches(&self) -> &'static [&'static str] {
        &[crate::types::kinds::JOB, crate::types::kinds::CRON_JOB]
    }
    async fn reconcile_once(&self) -> anyhow::Result<()> {
        let crons = self.crons().list(None)?;
        let jobs = self.jobs().list(None)?;
        let now = Utc::now();
        for cron in &crons {
            let res = if delete_requested(cron) {
                self.teardown(cron, &jobs)
            } else {
                self.converge(cron, &jobs, now)
            };
            if let Err(e) = res {
                pistack_types::tally::reconcile_failed("loom");
                tracing::warn!(cron = %cron.meta.id, error = %e, "loom: cronjob reconcile failed");
            }
        }
        Ok(())
    }
}
