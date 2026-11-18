// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use axum::{
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use pistack_types::{kind, ApiResult, Caller, ListQuery, Page};

use crate::types::FloatingIpSpec;

// The `crud!` body below expands with these in scope; they live in sibling files of this module.
use super::crud_deps::*;

crud!(
    FloatingIp,
    FloatingIpSpec,
    kind::FLOATING_IP,
    floating_ips,
    list_fips,
    create_fip,
    get_fip,
    update_fip,
    delete_fip,
    create_floating_ip,
    update_floating_ip,
    delete_floating_ip
);
