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

use super::vpc_of_subnet::vpc_of_subnet;
use crate::types::SubnetSpec;

// The `crud!` body below expands with these in scope; they live in sibling files of this module.
use super::crud_deps::*;

crud!(
    Subnet,
    SubnetSpec,
    kind::SUBNET,
    subnets,
    list_subnets,
    create_subnet,
    get_subnet,
    update_subnet,
    delete_subnet,
    create_subnet,
    update_subnet,
    delete_subnet,
    // A subnet is in a vpc through the network it sits on.
    vpc_of_subnet
);
