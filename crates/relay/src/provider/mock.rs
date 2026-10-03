// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! In-memory provider: records every call, allocates VIPs from a private
//! range, serves every `spec.provider`. Tests assert on [`MockProvider::calls`].

use super::{LbProvider, ListenerView, Realized};
use crate::types::{Balancer, Provider};
use async_trait::async_trait;
use parking_lot::Mutex;
use pistack_types::{ApiError, ApiResult, Id};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    EnsureBalancer {
        balancer: Id,
        vip: IpAddr,
    },
    DeleteBalancer(Id),
    EnsureListener {
        balancer: Id,
        listener: Id,
        port: u16,
        pool: Option<Id>,
        /// `address:port` of every member that should take traffic.
        members: Vec<String>,
    },
    DeleteListener {
        balancer: Id,
        listener: Id,
    },
}

#[derive(Default)]
struct State {
    calls: Vec<Call>,
    vips: BTreeMap<Id, IpAddr>,
    next: u32,
    /// When set, every call fails with this message.
    fail: Option<String>,
}

#[derive(Default)]
pub struct MockProvider {
    state: Mutex<State>,
}

impl MockProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn calls(&self) -> Vec<Call> {
        self.state.lock().calls.clone()
    }

    pub fn clear(&self) {
        self.state.lock().calls.clear();
    }

    /// Make every subsequent call fail (or succeed again with `None`).
    pub fn set_failure(&self, message: Option<&str>) {
        self.state.lock().fail = message.map(str::to_owned);
    }

    fn check(&self, s: &State) -> ApiResult<()> {
        match &s.fail {
            Some(m) => Err(ApiError::unavailable(m.clone())),
            None => Ok(()),
        }
    }
}

#[async_trait]
impl LbProvider for MockProvider {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn serves(&self, _provider: Provider) -> bool {
        true
    }

    async fn ensure_balancer(&self, lb: &Balancer) -> ApiResult<Realized> {
        let mut s = self.state.lock();
        self.check(&s)?;
        let vip = match lb.spec.vip_address {
            Some(a) => a,
            None => match s.vips.get(&lb.meta.id) {
                Some(v) => *v,
                None => {
                    s.next += 1;
                    let n = s.next;
                    IpAddr::V4(Ipv4Addr::new(198, 51, 100, (n % 254 + 1) as u8))
                }
            },
        };
        s.vips.insert(lb.meta.id.clone(), vip);
        s.calls.push(Call::EnsureBalancer {
            balancer: lb.meta.id.clone(),
            vip,
        });
        Ok(Realized {
            vip,
            provider_ref: format!("mock-{}", lb.meta.id.uuid),
        })
    }

    async fn delete_balancer(&self, id: &Id) -> ApiResult<()> {
        let mut s = self.state.lock();
        self.check(&s)?;
        s.vips.remove(id);
        s.calls.push(Call::DeleteBalancer(id.clone()));
        Ok(())
    }

    async fn ensure_listener(&self, view: &ListenerView) -> ApiResult<String> {
        let mut s = self.state.lock();
        self.check(&s)?;
        let mut members: Vec<String> = view
            .pool
            .as_ref()
            .map(|(_, m)| m.iter().map(|x| x.addr().to_string()).collect())
            .unwrap_or_default();
        members.sort();
        s.calls.push(Call::EnsureListener {
            balancer: view.balancer.meta.id.clone(),
            listener: view.listener.meta.id.clone(),
            port: view.listener.spec.port,
            pool: view.pool.as_ref().map(|(p, _)| p.meta.id.clone()),
            members,
        });
        Ok(format!("mock-{}", view.listener.meta.id.uuid))
    }

    async fn delete_listener(&self, balancer: &Id, listener: &Id) -> ApiResult<()> {
        let mut s = self.state.lock();
        self.check(&s)?;
        s.calls.push(Call::DeleteListener {
            balancer: balancer.clone(),
            listener: listener.clone(),
        });
        Ok(())
    }
}
