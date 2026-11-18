// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    routing::{get, post},
    Router,
};

use crate::http::app_state::AppState;
use crate::http::crud::crud;
use crate::http::index::index;
use crate::http::pod_exec::pod_exec;
use crate::http::pod_logs::pod_logs;
use crate::http::pod_sandbox::pod_sandbox;
use crate::http::rollback_deployment::rollback_deployment;
use crate::http::scale_deployment::scale_deployment;
use crate::http::scale_statefulset::scale_statefulset;
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

pub fn router(state: AppState) -> Router {
    let r = Router::new().route("/", get(index));
    let r = crud::<PodSpec>(r)
        .route("/pods/{id}/logs", get(pod_logs))
        .route("/pods/{id}/exec", post(pod_exec))
        .route("/pods/{id}/sandbox", get(pod_sandbox));
    let r = crud::<ReplicaSetSpec>(r);
    let r = crud::<DeploymentSpec>(r)
        .route("/deployments/{id}/scale", post(scale_deployment))
        .route("/deployments/{id}/rollback", post(rollback_deployment));
    let r = crud::<StatefulSetSpec>(r).route("/statefulsets/{id}/scale", post(scale_statefulset));
    let r = crud::<DaemonSetSpec>(r);
    let r = crud::<ServiceSpec>(r);
    let r = crud::<ConfigMapSpec>(r);
    let r = crud::<JobSpec>(r);
    let r = crud::<CronJobSpec>(r);
    let r = crud::<IngressSpec>(r);
    let r = crud::<AutoscalerSpec>(r);
    r.with_state(state)
}
