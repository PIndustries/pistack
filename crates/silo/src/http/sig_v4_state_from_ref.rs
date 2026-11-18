// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::sigv4::SigV4State;
use axum::extract::FromRef;

use super::app_state::AppState;

impl FromRef<AppState> for SigV4State {
    fn from_ref(s: &AppState) -> SigV4State {
        s.sigv4.clone()
    }
}
