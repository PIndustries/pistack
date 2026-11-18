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

use super::own_vpc::own_vpc;
use crate::types::NetworkSpec;

// The `crud!` body below expands with these in scope; they live in sibling files of this module.
use super::crud_deps::*;

crud!(
    Network,
    NetworkSpec,
    kind::NETWORK,
    networks,
    list_networks,
    create_network,
    get_network,
    update_network,
    delete_network,
    create_network,
    update_network,
    delete_network,
    own_vpc
);
