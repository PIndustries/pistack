// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::crud_deps::*;
use crate::types;
use crate::types::RouterSpec;
use pistack_types::kind;

crud!(
    types::Router,
    RouterSpec,
    kind::ROUTER,
    routers,
    list_routers,
    create_router,
    get_router,
    update_router,
    delete_router,
    create_router,
    update_router,
    delete_router
);
