// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::catalog::RESERVED_PREFIX;
use crate::client::Client;
use crate::series::Series;
use crate::types::Sample;
use axum::{
    extract::{Extension, Query, State},
    Json,
};
use pistack_types::{Caller, Id};

use super::sample_query::SampleQuery;

pub(crate) async fn list_samples(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<SampleQuery>,
) -> Json<Vec<Sample>> {
    let metric = q.metric.as_deref().filter(|m| !m.is_empty());
    let mut samples = client.list_samples(metric);
    if !caller.is_admin() {
        // Host series and other projects' machines are not theirs to see.
        let own = caller.project.as_ref().map(Id::to_string);
        samples.retain(|s| {
            let series = Series::parse(&s.metric);
            !series.name.starts_with(RESERVED_PREFIX)
                || (own.is_some() && series.labels.get("project") == own.as_ref())
        });
    }
    Json(samples)
}
