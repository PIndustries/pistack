// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::kinds;
use pistack_types::Spec;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::concurrency_policy::ConcurrencyPolicy;
use super::job_spec::JobSpec;
use super::one::one;
use super::three::three;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CronJobSpec {
    /// Five-field cron expression, UTC. See [`crate::cron`].
    pub schedule: String,
    pub job_template: JobSpec,
    #[serde(default)]
    pub suspend: bool,
    #[serde(default)]
    pub concurrency_policy: ConcurrencyPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_deadline_seconds: Option<u64>,
    #[serde(default = "three")]
    pub successful_jobs_history_limit: u32,
    #[serde(default = "one")]
    pub failed_jobs_history_limit: u32,
}

impl Spec for CronJobSpec {
    const KIND: &'static str = kinds::CRON_JOB;
}
