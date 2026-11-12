// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! In-memory runtime. Addresses are `10.0.0.N`, allocated in start order and
//! stable for the life of the machine. Mirrors `forge::MockDriver`.

use super::{MachineRequest, MachineRuntime, PowerState, RunningMachine};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct MockRuntime {
    inner: Mutex<State>,
}

#[derive(Default)]
struct State {
    machines: BTreeMap<Id, RunningMachine>,
    probes: BTreeMap<Id, Vec<crate::probe::ContainerProbes>>,
    next_addr: u32,
}

impl MockRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> ApiResult<std::sync::MutexGuard<'_, State>> {
        self.inner
            .lock()
            .map_err(|_| ApiError::internal("mock runtime state poisoned"))
    }
}

#[async_trait]
impl MachineRuntime for MockRuntime {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn start(&self, req: &MachineRequest) -> ApiResult<RunningMachine> {
        let mut st = self.lock()?;
        st.probes.insert(
            req.id.clone(),
            crate::probe::probes_in_user_data(req.user_data.as_deref()),
        );
        if let Some(rt) = st.machines.get_mut(&req.id) {
            rt.power = PowerState::On;
            return Ok(rt.clone());
        }
        st.next_addr += 1;
        let n = st.next_addr;
        if n > 254 {
            return Err(ApiError::unavailable(
                "mock runtime: address pool exhausted",
            ));
        }
        let rt = RunningMachine {
            id: req.id.clone(),
            addresses: vec![format!("10.0.0.{n}")],
            power: PowerState::On,
            message: String::new(),
        };
        st.machines.insert(req.id.clone(), rt.clone());
        Ok(rt)
    }

    async fn stop(&self, id: &Id) -> ApiResult<()> {
        let mut st = self.lock()?;
        match st.machines.get_mut(id) {
            Some(rt) => {
                rt.power = PowerState::Off;
                Ok(())
            }
            None => Err(ApiError::not_found(format!(
                "mock runtime: machine {id} not here"
            ))),
        }
    }

    async fn delete(&self, id: &Id) -> ApiResult<()> {
        let mut st = self.lock()?;
        st.machines.remove(id);
        st.probes.remove(id);
        Ok(())
    }

    async fn container_probes(&self, id: &Id) -> Vec<crate::probe::ContainerProbes> {
        self.lock()
            .map(|st| st.probes.get(id).cloned().unwrap_or_default())
            .unwrap_or_default()
    }

    async fn list(&self) -> ApiResult<Vec<RunningMachine>> {
        Ok(self.lock()?.machines.values().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MachineKind;
    use pistack_types::{kind, Resources};

    fn req(name: &str) -> MachineRequest {
        MachineRequest {
            id: Id::new(kind::MACHINE),
            name: name.into(),
            kind: MachineKind::Vm,
            image: "img".into(),
            image_source: None,
            resources: Resources {
                vcpus: 1,
                memory_mib: 1024,
                disk_gib: 10,
                gpus: 0,
            },
            networks: vec![],
            nics: vec![],
            volumes: vec![],
            user_data: None,
            ssh_keys: vec![],
            overlay: None,
            bmc_address: None,
            bmc_user: None,
        }
    }

    #[tokio::test]
    async fn lifecycle() {
        let rt = MockRuntime::new();
        let a = req("a");
        let b = req("b");
        assert_eq!(rt.start(&a).await.unwrap().addresses, vec!["10.0.0.1"]);
        assert_eq!(rt.start(&b).await.unwrap().addresses, vec!["10.0.0.2"]);
        assert_eq!(rt.start(&a).await.unwrap().addresses, vec!["10.0.0.1"]);
        rt.stop(&a.id).await.unwrap();
        assert_eq!(
            rt.list()
                .await
                .unwrap()
                .iter()
                .filter(|m| m.power == PowerState::Off)
                .count(),
            1
        );
        rt.delete(&a.id).await.unwrap();
        rt.delete(&a.id).await.unwrap();
        assert!(rt.stop(&a.id).await.is_err());
        assert_eq!(rt.list().await.unwrap().len(), 1);
    }
}
