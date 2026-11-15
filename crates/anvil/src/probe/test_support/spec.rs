// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::spec::http_get_spec::HttpGetSpec;
use crate::probe::spec::probe_spec::ProbeSpec;

pub(in crate::probe) fn spec(period: u32, delay: u32) -> ProbeSpec {
    ProbeSpec {
        http_get: Some(HttpGetSpec {
            path: "/health".into(),
            port: 8080,
            host: None,
            scheme: None,
        }),
        tcp_socket: None,
        exec: None,
        initial_delay_seconds: delay,
        period_seconds: period,
        timeout_seconds: 1,
        success_threshold: 1,
        failure_threshold: 3,
    }
}
