// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use crate::controllers::Controller;
#[async_trait::async_trait]
impl Controller for PodController {
    fn name(&self) -> &'static str {
        "pod"
    }
    fn watches(&self) -> &'static [&'static str] {
        &[
            crate::types::kinds::POD,
            pistack_types::kind::MACHINE,
            pistack_types::kind::VOLUME,
        ]
    }
    async fn reconcile_once(&self) -> anyhow::Result<()> {
        let pods = self.pods().list(None)?;
        let machines = self.machines().list(None)?;
        for pod in &pods {
            let res = if crate::types::delete_requested(pod) {
                self.teardown(pod, &machines).await
            } else {
                self.converge(pod, &machines).await
            };
            if let Err(e) = res {
                pistack_types::tally::reconcile_failed("loom");
                tracing::warn!(pod = %pod.meta.id, error = %e, "loom: pod reconcile failed");
            }
        }
        Ok(())
    }
}
