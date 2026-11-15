// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use crate::types::{Pod, MACHINE_POD_LABEL};
use forge::Machine;
impl PodController {
    pub(in crate::controllers::pod) fn machine_of<'a>(
        &self,
        pod: &Pod,
        machines: &'a [Machine],
    ) -> Option<&'a Machine> {
        let pid = pod.meta.id.to_string();
        pod.status
            .machine
            .as_ref()
            .and_then(|mid| machines.iter().find(|m| &m.meta.id == mid))
            .or_else(|| {
                machines
                    .iter()
                    .find(|m| m.meta.labels.get(MACHINE_POD_LABEL) == Some(&pid))
            })
    }
}
