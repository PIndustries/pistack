// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

// ---- kinds and well-known keys ------------------------------------------------

use super::cron_job_spec::CronJobSpec;
use super::cron_job_status::CronJobStatus;

pub type CronJob = Resource<CronJobSpec, CronJobStatus>;
