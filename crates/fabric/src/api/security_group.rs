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

use crate::types::SecurityGroupSpec;

// The `crud!` body below expands with these in scope; they live in sibling files of this module.
use super::crud_deps::*;

crud!(
    SecurityGroup,
    SecurityGroupSpec,
    kind::SECGROUP,
    security_groups,
    list_sgs,
    create_sg,
    get_sg,
    update_sg,
    delete_sg,
    create_security_group,
    update_security_group,
    delete_security_group
);
