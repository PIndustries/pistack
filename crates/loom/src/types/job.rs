// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

// ---- kinds and well-known keys ------------------------------------------------

use super::job_spec::JobSpec;
use super::job_status::JobStatus;

pub type Job = Resource<JobSpec, JobStatus>;
