// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{
    ServiceEndpointView, ServicePortView, ServiceRecord, ServiceStatusView, ServiceView,
};
use pistack_types::{Id, Meta};

pub(in crate::overlay) fn service(
    project: &Id,
    ip: &str,
    endpoints: &[&str],
    ports: &[(&str, u16, Option<u16>)],
) -> ServiceRecord {
    ServiceRecord {
        meta: Meta::new(
            "svc",
            format!("web-{}", ip.replace('.', "-")),
            Some(project.clone()),
        ),
        spec: ServiceView {
            ports: ports
                .iter()
                .map(|(protocol, port, target_port)| ServicePortView {
                    protocol: (*protocol).into(),
                    port: *port,
                    target_port: *target_port,
                })
                .collect(),
        },
        status: ServiceStatusView {
            cluster_ip: Some(ip.into()),
            endpoints: endpoints
                .iter()
                .map(|ip| ServiceEndpointView { ip: (*ip).into() })
                .collect(),
        },
    }
}
