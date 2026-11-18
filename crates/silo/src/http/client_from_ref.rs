// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::extract::FromRef;

use super::app_state::AppState;

impl FromRef<AppState> for Client {
    fn from_ref(s: &AppState) -> Client {
        s.client.clone()
    }
}
