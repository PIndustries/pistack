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

use crate::types::PortSpec;

// The `crud!` body below expands with these in scope; they live in sibling files of this module.
use super::crud_deps::*;

crud!(
    Port,
    PortSpec,
    kind::PORT,
    ports,
    list_ports,
    create_port,
    get_port,
    update_port,
    delete_port,
    create_port,
    update_port,
    delete_port
);
