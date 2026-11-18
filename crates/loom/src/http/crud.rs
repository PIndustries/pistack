// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{routing::get, Router};

use crate::http::api::Api;
use crate::http::app_state::AppState;
use crate::http::create::create;
use crate::http::delete::delete;
use crate::http::get_one::get_one;
use crate::http::list::list;
use crate::http::update::update;

pub(crate) fn crud<A: Api>(r: Router<AppState>) -> Router<AppState> {
    r.route(&format!("/{}", A::NOUN), get(list::<A>).post(create::<A>))
        .route(
            &format!("/{}/{{id}}", A::NOUN),
            get(get_one::<A>).put(update::<A>).delete(delete::<A>),
        )
}
