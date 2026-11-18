// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Collection;

use crate::http::api::Api;
use crate::http::app_state::AppState;
use crate::http::res::Res;

pub(crate) fn col<A: Api>(s: &AppState) -> Collection<Res<A>> {
    s.store.collection::<Res<A>>()
}
