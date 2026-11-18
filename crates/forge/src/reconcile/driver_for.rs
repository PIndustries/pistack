// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::driver::HostDriver;
use crate::types::Machine;
use pistack_types::{ApiError, Id};
use std::sync::Arc;

use super::reconciler::Reconciler;

impl Reconciler {
    pub(super) fn driver_for(&self, m: &Machine) -> Result<(Id, Arc<dyn HostDriver>), ApiError> {
        let hid = m
            .status
            .host
            .clone()
            .ok_or_else(|| ApiError::internal(format!("machine {} has no host", m.meta.id)))?;
        let d = self
            .drivers
            .get(&hid)
            .ok_or_else(|| ApiError::unavailable(format!("no driver registered for host {hid}")))?;
        Ok((hid, d))
    }
}
