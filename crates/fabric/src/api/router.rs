// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::middleware;
use axum::routing::{get, post, put};

use super::add_rule::add_rule;
use super::associate_fip::associate_fip;
use super::create_vpc::create_vpc;
use super::create_vpc_subnet::create_vpc_subnet;
use super::crud_generated::*;
use super::delete_vpc::delete_vpc;
use super::delete_vpc_subnet::delete_vpc_subnet;
use super::disassociate_fip::disassociate_fip;
use super::get_vpc::get_vpc;
use super::info::info;
use super::list_vpc_subnets::list_vpc_subnets;
use super::list_vpcs::list_vpcs;
use super::remove_rule::remove_rule;
use super::require_caller::require_caller;
use super::set_router_interfaces::set_router_interfaces;
use super::update_vpc::update_vpc;

pub fn router(client: Client) -> axum::Router {
    axum::Router::new()
        .route("/", get(info))
        .route("/vpcs", get(list_vpcs).post(create_vpc))
        .route(
            "/vpcs/{id}",
            get(get_vpc).put(update_vpc).delete(delete_vpc),
        )
        .route(
            "/vpcs/{id}/subnets",
            get(list_vpc_subnets).post(create_vpc_subnet),
        )
        .route(
            "/vpcs/{id}/subnets/{subnet}",
            axum::routing::delete(delete_vpc_subnet),
        )
        .route("/networks", get(list_networks).post(create_network))
        .route(
            "/networks/{id}",
            get(get_network).put(update_network).delete(delete_network),
        )
        .route("/subnets", get(list_subnets).post(create_subnet))
        .route(
            "/subnets/{id}",
            get(get_subnet).put(update_subnet).delete(delete_subnet),
        )
        .route("/ports", get(list_ports).post(create_port))
        .route(
            "/ports/{id}",
            get(get_port).put(update_port).delete(delete_port),
        )
        .route("/routers", get(list_routers).post(create_router))
        .route(
            "/routers/{id}",
            get(get_router).put(update_router).delete(delete_router),
        )
        .route("/routers/{id}/interfaces", put(set_router_interfaces))
        .route("/security-groups", get(list_sgs).post(create_sg))
        .route(
            "/security-groups/{id}",
            get(get_sg).put(update_sg).delete(delete_sg),
        )
        .route("/security-groups/{id}/rules", post(add_rule))
        .route(
            "/security-groups/{id}/rules/{rule}",
            axum::routing::delete(remove_rule),
        )
        .route("/floating-ips", get(list_fips).post(create_fip))
        .route(
            "/floating-ips/{id}",
            get(get_fip).put(update_fip).delete(delete_fip),
        )
        .route("/floating-ips/{id}/associate", post(associate_fip))
        .route("/floating-ips/{id}/disassociate", post(disassociate_fip))
        .layer(middleware::from_fn(require_caller))
        .with_state(client)
}
