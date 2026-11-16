// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::{FloatingIp, Port, Router, Subnet};

pub(in crate::overlay) fn collides_with_router_ip(
    fip: &FloatingIp,
    floating: &str,
    port: &Port,
    routers: &[Router],
    subnets: &[Subnet],
) -> bool {
    routers.iter().any(|router| {
        let named = fip.status.router.as_ref() == Some(&router.meta.id);
        let on_port = router.spec.interfaces.iter().any(|sid| {
            subnets.iter().any(|subnet| {
                &subnet.meta.id == sid
                    && (port.spec.subnet.as_ref() == Some(&subnet.meta.id)
                        || port
                            .spec
                            .fixed_ips
                            .iter()
                            .any(|ip| subnet.spec.cidr.contains(ip)))
            })
        });
        (named || on_port)
            && router
                .status
                .external_ip
                .map(|ip| ip.to_string())
                .as_deref()
                == Some(floating)
    })
}
