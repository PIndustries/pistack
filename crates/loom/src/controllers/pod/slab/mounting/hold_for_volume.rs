// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use crate::controllers::pod::slab::machine_live;
use crate::types::Pod;
use forge::Machine;
use pistack_store::StoreError;
impl PodController {
    pub(in crate::controllers::pod) fn hold_for_volume(
        &self,
        pod: &Pod,
        machine: &Machine,
        message: &str,
    ) -> Result<(), StoreError> {
        if !machine_live(machine) {
            let current = machine
                .meta
                .annotations
                .get(forge::WAIT_VOLUME_ANNOTATION)
                .map(String::as_str);
            if current != Some(message) {
                match self.machines().modify(&machine.meta.id, |m| {
                    m.meta
                        .annotations
                        .insert(forge::WAIT_VOLUME_ANNOTATION.into(), message.to_string());
                    Ok(())
                }) {
                    Ok(_) | Err(StoreError::NotFound(_)) => {}
                    Err(e) => return Err(e),
                }
            }
        }
        self.mark_waiting(pod, message)
    }
}
#[cfg(test)]
#[path = "hold_for_volume_tests.rs"]
mod hold_for_volume_tests;
