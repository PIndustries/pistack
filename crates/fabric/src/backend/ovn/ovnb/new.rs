// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::super::super::ovsdb;
use super::super::super::ovsdb::Transport;
use super::super::pub_consts::NB_ENV;
use super::super::{
    bad_endpoint::BadEndpoint, chassis_from_env::chassis_from_env, ovn_backend::OvnBackend,
};
use std::sync::Arc;

impl OvnBackend {
    pub fn new(nb_endpoint: impl Into<String>) -> Self {
        let nb_endpoint = nb_endpoint.into();
        let nb: Arc<dyn Transport> = match ovsdb::Endpoint::parse(&nb_endpoint) {
            Ok(ep) => Arc::new(ovsdb::Client::new(ep)),
            Err(e) => Arc::new(BadEndpoint(format!("{NB_ENV}: {}", e.message))),
        };
        Self::with_transport(nb_endpoint, nb).with_gateway_chassis(chassis_from_env())
    }
}
