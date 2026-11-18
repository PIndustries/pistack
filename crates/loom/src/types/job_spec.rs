// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::kinds;
use pistack_types::Spec;
use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::one::one;
use super::pod_template::PodTemplate;
use super::six::six;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobSpec {
    pub template: PodTemplate,
    #[serde(default = "one")]
    pub completions: u32,
    #[serde(default = "one")]
    pub parallelism: u32,
    #[serde(default = "six")]
    pub backoff_limit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_deadline_seconds: Option<u64>,
    /// Delete the job (and its pods) this long after it finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_seconds_after_finished: Option<u64>,
    #[serde(default)]
    pub suspend: bool,
}

impl Spec for JobSpec {
    const KIND: &'static str = kinds::JOB;
}
