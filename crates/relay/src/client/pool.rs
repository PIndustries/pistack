// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Pool rules: what a health monitor must carry, which persistence a
//! protocol allows, and the status a new member starts from.

use crate::types::{
    MemberSpec, MonitorType, OperatingStatus, Pool, PoolProtocol, PoolSpec, SessionPersistence,
};
use pistack_types::{ApiError, ApiResult};

pub(super) fn validate_pool(spec: &PoolSpec) -> ApiResult<()> {
    if let Some(hm) = &spec.health_monitor {
        if hm.interval == 0 || hm.timeout == 0 {
            return Err(ApiError::bad_request(
                "health monitor interval and timeout must be greater than zero",
            ));
        }
        if hm.rise == 0 || hm.fall == 0 {
            return Err(ApiError::bad_request(
                "health monitor rise and fall must be greater than zero",
            ));
        }
        if hm.timeout > hm.interval {
            return Err(ApiError::bad_request(
                "health monitor timeout must not exceed the interval",
            ));
        }
        if hm.kind != MonitorType::Tcp {
            if !hm.path.starts_with('/') {
                return Err(ApiError::bad_request(
                    "health monitor path must start with '/'",
                ));
            }
            if hm.expected_codes.is_empty() {
                return Err(ApiError::bad_request(
                    "health monitor needs at least one expected status code",
                ));
            }
        }
    }
    if let Some(SessionPersistence::AppCookie { cookie }) = &spec.session_persistence {
        if cookie.trim().is_empty() {
            return Err(ApiError::bad_request(
                "app_cookie persistence needs a cookie name",
            ));
        }
    }
    if spec.protocol == PoolProtocol::Udp
        && spec.session_persistence.is_some()
        && !matches!(spec.session_persistence, Some(SessionPersistence::SourceIp))
    {
        return Err(ApiError::bad_request(
            "udp pools support source_ip persistence only",
        ));
    }
    Ok(())
}

pub(super) fn initial_status(pool: &Pool, spec: &MemberSpec) -> OperatingStatus {
    if !spec.admin_up {
        OperatingStatus::Draining
    } else if pool.spec.health_monitor.is_none() {
        OperatingStatus::NoMonitor
    } else {
        OperatingStatus::Offline
    }
}
