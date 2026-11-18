// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::Json;

use crate::http::api::Api;
use crate::types::AutoscalerSpec;
use crate::types::ConfigMapSpec;
use crate::types::CronJobSpec;
use crate::types::DaemonSetSpec;
use crate::types::DeploymentSpec;
use crate::types::IngressSpec;
use crate::types::JobSpec;
use crate::types::PodSpec;
use crate::types::ReplicaSetSpec;
use crate::types::ServiceSpec;
use crate::types::StatefulSetSpec;

pub(crate) async fn index() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "module": "loom",
        "resources": [
            PodSpec::NOUN, ReplicaSetSpec::NOUN, DeploymentSpec::NOUN, StatefulSetSpec::NOUN,
            DaemonSetSpec::NOUN, ServiceSpec::NOUN, ConfigMapSpec::NOUN, JobSpec::NOUN,
            CronJobSpec::NOUN, IngressSpec::NOUN, AutoscalerSpec::NOUN,
        ],
    }))
}
