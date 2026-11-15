// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use crate::controllers::{delete_quietly, patch_status, request_delete};
use crate::types::Pod;
use forge::Machine;
impl PodController {
    pub(in crate::controllers::pod) async fn teardown(
        &self,
        pod: &Pod,
        machines: &[Machine],
    ) -> anyhow::Result<()> {
        if let Some(m) = self.machine_of(pod, machines) {
            request_delete(&self.machines(), &m.meta.id)?;
            patch_status(&self.pods(), &pod.meta.id, |s| {
                s.message = "deleting machine".into();
            })?;
            return Ok(());
        }
        delete_quietly(&self.pods(), &pod.meta.id)?;
        tracing::info!(pod = %pod.meta.id, name = %pod.meta.name, "loom: pod deleted");
        Ok(())
    }
}
