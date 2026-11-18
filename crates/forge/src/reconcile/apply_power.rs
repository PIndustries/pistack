// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{DesiredPower, Machine, MachineStatus, PowerState};
use pistack_types::ApiError;

use super::copy_network;
use super::note_network;
use super::reconciler::Reconciler;

impl Reconciler {
    pub(super) async fn apply_power(&self, m: Machine) -> anyhow::Result<()> {
        let want_restart = m.spec.restarts > m.status.restarts;
        let want_off = m.spec.power == DesiredPower::Off;
        let is_on = m.status.power == PowerState::On;
        if !want_restart && want_off != is_on {
            return Ok(());
        }
        let (_, driver) = match self.driver_for(&m) {
            Ok(x) => x,
            Err(e) => {
                let msg = e.message.clone();
                self.patch(&m.meta.id, |s| s.message = msg.clone()).await?;
                return Ok(());
            }
        };
        let restarts = m.spec.restarts;
        let result: Result<MachineStatus, ApiError> = async {
            let mut st = m.status.clone();
            if want_restart || want_off {
                if is_on {
                    driver.stop(&m.meta.id).await?;
                }
                st.power = PowerState::Off;
            }
            let network_message = if !want_off {
                crate::nic::ensure_ports(&self.store, &m).await?;
                let rt = driver.start(&m).await?;
                st.power = rt.power;
                st.addresses = rt.addresses;
                rt.message
            } else {
                String::new()
            };
            st.restarts = restarts;
            st.message.clear();
            note_network(&mut st, &network_message);
            Ok(st)
        }
        .await;
        match result {
            Ok(st) => {
                self.patch(&m.meta.id, |s| {
                    s.power = st.power;
                    s.addresses = st.addresses.clone();
                    s.restarts = st.restarts;
                    s.message = st.message.clone();
                    s.set_condition("Running", st.power == PowerState::On, "PowerApplied", "");
                    copy_network(s, &st);
                })
                .await?;
            }
            Err(e) => {
                let msg = e.message.clone();
                self.patch(&m.meta.id, |s| s.message = msg.clone()).await?;
            }
        }
        Ok(())
    }
}
