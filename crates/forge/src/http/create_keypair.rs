// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::keypair;
use crate::types::new_keypair;
use axum::{extract::State, response::Response};
use pistack_types::{ApiError, ApiResult};
use serde::Deserialize;
use std::collections::BTreeMap;

use super::app_state::AppState;
use super::body::Body;
use super::created::created;
use super::keypair_created::KeypairCreated;
use super::keypairs::keypairs;
use super::scope::Scope;
use super::validate_name::validate_name;

/// What a create takes. With `public_key` the key is imported; without one
/// an `ed25519` pair is made and its private half returned once.
#[derive(Debug, Deserialize)]
pub struct CreateKeypair {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub public_key: Option<String>,
}

pub(crate) async fn create_keypair(
    State(s): State<AppState>,
    scope: Scope,
    Body(body): Body<CreateKeypair>,
) -> ApiResult<Response> {
    let project = scope
        .project
        .clone()
        .ok_or_else(|| ApiError::forbidden("a project is required to hold a key pair"))?;
    validate_name(&body.name)?;
    let (key, private_key) = match &body.public_key {
        Some(line) => (keypair::parse(line)?, None),
        None => {
            let made = keypair::generate_ed25519(&body.name);
            (made.public, Some(made.private_openssh))
        }
    };
    let mut kp = new_keypair(body.name, Some(project), key);
    kp.meta.description = body.description;
    kp.meta.labels = body.labels;
    let kp = keypairs(&s).create(kp)?;
    Ok(created(KeypairCreated {
        keypair: kp,
        private_key,
    }))
}
