// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Machine, PowerState};
use async_trait::async_trait;
use parking_lot::Mutex;
use pistack_types::{ApiError, ApiResult, Id};

use super::host_driver::HostDriver;
use super::machine_runtime::MachineRuntime;
use super::mock_state::MockState;

/// In-memory driver. Addresses are allocated from `10.0.0.1` upwards and
/// are stable for the life of a machine.
#[derive(Default)]
pub struct MockDriver {
    pub(crate) inner: Mutex<MockState>,
    /// Set to make `rebuild` fail once, as a host whose disk cannot be
    /// written does. Tests use it.
    pub(crate) fail_rebuild: std::sync::atomic::AtomicBool,
}

impl MockDriver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of machines the mock knows about (running or stopped).
    pub fn len(&self) -> usize {
        self.inner.lock().machines.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The next `rebuild` fails and clears this. Nothing else notices.
    pub fn fail_next_rebuild(&self) {
        self.fail_rebuild
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[async_trait]
impl HostDriver for MockDriver {
    async fn start(&self, m: &Machine) -> ApiResult<MachineRuntime> {
        let mut st = self.inner.lock();
        if let Some(rt) = st.machines.get_mut(&m.meta.id) {
            rt.power = PowerState::On;
            return Ok(rt.clone());
        }
        st.next_addr += 1;
        let n = st.next_addr;
        if n > 254 {
            return Err(ApiError::unavailable("mock driver: address pool exhausted"));
        }
        let rt = MachineRuntime {
            id: m.meta.id.clone(),
            addresses: vec![format!("10.0.0.{n}")],
            power: PowerState::On,
            message: String::new(),
        };
        st.machines.insert(m.meta.id.clone(), rt.clone());
        Ok(rt)
    }

    async fn stop(&self, id: &Id) -> ApiResult<()> {
        let mut st = self.inner.lock();
        match st.machines.get_mut(id) {
            Some(rt) => {
                rt.power = PowerState::Off;
                Ok(())
            }
            None => Err(ApiError::not_found(format!(
                "mock driver: machine {id} not running here"
            ))),
        }
    }

    async fn delete(&self, id: &Id) -> ApiResult<()> {
        self.inner.lock().machines.remove(id);
        Ok(())
    }

    /// The record's power is reset and its address kept: a rebuild replaces
    /// the disk, not the machine's place on the network.
    async fn rebuild(&self, m: &Machine) -> ApiResult<MachineRuntime> {
        if self
            .fail_rebuild
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            return Err(ApiError::internal(
                "mock driver: the disk could not be written",
            ));
        }
        self.start(m).await
    }

    async fn list(&self) -> ApiResult<Vec<MachineRuntime>> {
        Ok(self.inner.lock().machines.values().cloned().collect())
    }
}
