// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Autoscaler controller: read gauge samples and set replica counts.
//!
//! Each Autoscaler names a Deployment or StatefulSet, a metric, and a
//! target value. One reconcile pass takes the latest sample for that
//! metric, computes [`desired_replicas`], writes the target's
//! `spec.replicas` when it differs, and records the decision on the
//! autoscaler's status.

use super::{delete_quietly, patch_status, Controller};
use crate::types::{
    delete_requested, desired_replicas, set_condition, Autoscaler, Deployment, ScaleTargetKind,
    StatefulSet,
};
use async_trait::async_trait;
use pistack_store::{Collection, Store};
use pistack_types::Id;

pub struct AutoscalerController {
    store: Store,
}

impl AutoscalerController {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    fn autoscalers(&self) -> Collection<Autoscaler> {
        self.store.collection()
    }

    fn deployments(&self) -> Collection<Deployment> {
        self.store.collection()
    }

    fn statefulsets(&self) -> Collection<StatefulSet> {
        self.store.collection()
    }

    fn converge(&self, a: &Autoscaler) -> anyhow::Result<()> {
        if delete_requested(a) {
            delete_quietly(&self.autoscalers(), &a.meta.id)?;
            tracing::info!(scaler = %a.meta.id, name = %a.meta.name, "loom: autoscaler deleted");
            return Ok(());
        }

        let metric = a.spec.metric.trim();
        let sample = if metric.is_empty() {
            None
        } else {
            gauge::samples().latest(metric)
        };

        let (target_id, current) = match self.resolve_target(a)? {
            Some(v) => v,
            None => {
                patch_status(&self.autoscalers(), &a.meta.id, |s| {
                    s.message = "target is not available".into();
                    s.last_metric = sample.as_ref().map(|x| x.value);
                    set_condition(
                        &mut s.conditions,
                        "Ready",
                        false,
                        "TargetMissing",
                        "target is not available",
                    );
                })?;
                return Ok(());
            }
        };

        let Some(sample) = sample else {
            patch_status(&self.autoscalers(), &a.meta.id, |s| {
                s.current_replicas = current;
                s.message = "no sample for metric".into();
                s.last_metric = None;
                set_condition(
                    &mut s.conditions,
                    "Ready",
                    false,
                    "NoSample",
                    "no sample for metric",
                );
            })?;
            return Ok(());
        };

        let want = desired_replicas(
            current,
            sample.value,
            a.spec.target_value,
            a.spec.min_replicas,
            a.spec.max_replicas,
        );

        if want != current {
            match a.spec.target_kind {
                ScaleTargetKind::Deployment => {
                    self.deployments().modify(&target_id, |d| {
                        d.spec.replicas = want;
                        Ok(())
                    })?;
                }
                ScaleTargetKind::StatefulSet => {
                    self.statefulsets().modify(&target_id, |sts| {
                        sts.spec.replicas = want;
                        Ok(())
                    })?;
                }
            }
            tracing::info!(
                scaler = %a.meta.id,
                target = %target_id,
                from = current,
                to = want,
                metric = %a.spec.metric,
                value = sample.value,
                "loom: autoscaler scaled target"
            );
        }

        patch_status(&self.autoscalers(), &a.meta.id, |s| {
            s.desired_replicas = want;
            s.current_replicas = want;
            s.last_metric = Some(sample.value);
            s.message = if want == current {
                "replicas match metric".into()
            } else {
                format!("scaled to {want}")
            };
            let msg = s.message.clone();
            set_condition(&mut s.conditions, "Ready", true, "Scaled", &msg);
        })?;
        Ok(())
    }

    fn resolve_target(&self, a: &Autoscaler) -> anyhow::Result<Option<(Id, u32)>> {
        let name = a.spec.target_name.trim();
        if name.is_empty() {
            return Ok(None);
        }
        let project = a.meta.project.as_ref();
        match a.spec.target_kind {
            ScaleTargetKind::Deployment => {
                let d = match self.deployments().resolve(project, name) {
                    Ok(d) => d,
                    Err(_) => return Ok(None),
                };
                if delete_requested(&d) {
                    return Ok(None);
                }
                Ok(Some((d.meta.id.clone(), d.spec.replicas)))
            }
            ScaleTargetKind::StatefulSet => {
                let sts = match self.statefulsets().resolve(project, name) {
                    Ok(sts) => sts,
                    Err(_) => return Ok(None),
                };
                if delete_requested(&sts) {
                    return Ok(None);
                }
                Ok(Some((sts.meta.id.clone(), sts.spec.replicas)))
            }
        }
    }
}

#[async_trait]
impl Controller for AutoscalerController {
    fn name(&self) -> &'static str {
        "autoscaler"
    }

    fn watches(&self) -> &'static [&'static str] {
        &[
            crate::types::kinds::AUTOSCALER,
            crate::types::kinds::DEPLOYMENT,
            crate::types::kinds::STATEFUL_SET,
        ]
    }

    async fn reconcile_once(&self) -> anyhow::Result<()> {
        let rows = self.autoscalers().list(None)?;
        for a in rows {
            if let Err(e) = self.converge(&a) {
                tracing::warn!(
                    scaler = %a.meta.id,
                    error = %e,
                    "loom: autoscaler converge failed"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        desired_replicas, AutoscalerSpec, Container, DeploymentSpec, DeploymentStrategy, PodSpec,
        PodTemplate, ScaleTargetKind, Selector,
    };
    use chrono::Utc;
    use gauge::Sample;
    use pistack_types::Resource;
    use std::collections::BTreeMap;

    fn deploy(name: &str, replicas: u32) -> Deployment {
        let mut labels = BTreeMap::new();
        labels.insert("app".into(), name.into());
        Resource::new(
            name,
            None,
            DeploymentSpec {
                replicas,
                selector: Selector {
                    match_labels: labels.clone(),
                    match_expressions: vec![],
                },
                template: PodTemplate {
                    labels,
                    annotations: BTreeMap::new(),
                    spec: PodSpec {
                        containers: vec![Container {
                            name: "app".into(),
                            image: "app:1".into(),
                            command: vec![],
                            args: vec![],
                            env: vec![],
                            ports: vec![],
                            resources: Default::default(),
                            volume_mounts: vec![],
                            working_dir: String::new(),
                            liveness_probe: None,
                            readiness_probe: None,
                        }],
                        volumes: vec![],
                        restart_policy: Default::default(),
                        node_selector: BTreeMap::new(),
                        service_account: String::new(),
                        networks: vec![],
                    },
                },
                strategy: DeploymentStrategy::default(),
                revision_history_limit: 3,
                paused: false,
            },
        )
    }

    fn scaler(target: &str, metric: &str, target_value: f64, min: u32, max: u32) -> Autoscaler {
        Resource::new(
            "cpu",
            None,
            AutoscalerSpec {
                target_kind: ScaleTargetKind::Deployment,
                target_name: target.into(),
                metric: metric.into(),
                target_value,
                min_replicas: min,
                max_replicas: max,
            },
        )
    }

    #[test]
    fn scale_decision_clamps_and_ratios() {
        assert_eq!(desired_replicas(2, 80.0, 40.0, 1, 10), 4);
        assert_eq!(desired_replicas(4, 20.0, 40.0, 1, 10), 2);
        assert_eq!(desired_replicas(1, 100.0, 50.0, 1, 3), 2);
        assert_eq!(desired_replicas(0, 90.0, 45.0, 1, 5), 2);
        assert_eq!(desired_replicas(5, 10.0, 40.0, 3, 10), 3);
        assert_eq!(desired_replicas(2, f64::NAN, 40.0, 1, 10), 2);
    }

    #[test]
    fn controller_scales_deployment_from_sample() {
        let store = Store::in_memory().unwrap();
        let d = store
            .collection::<Deployment>()
            .create(deploy("web", 2))
            .unwrap();
        let a = store
            .collection::<Autoscaler>()
            .create(scaler("web", "cpu.usage", 40.0, 1, 10))
            .unwrap();

        gauge::samples().push(Sample {
            metric: "cpu.usage".into(),
            value: 80.0,
            timestamp: Utc::now(),
        });

        let c = AutoscalerController::new(store.clone());
        c.converge(&a).unwrap();

        let d = store.collection::<Deployment>().get(&d.meta.id).unwrap();
        assert_eq!(d.spec.replicas, 4);

        let a = store.collection::<Autoscaler>().get(&a.meta.id).unwrap();
        assert_eq!(a.status.desired_replicas, 4);
        assert_eq!(a.status.last_metric, Some(80.0));
    }
}
