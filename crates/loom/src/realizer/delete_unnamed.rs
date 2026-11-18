// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use relay::Client;

pub(crate) async fn delete_unnamed(
    client: &Client,
    label: &str,
    id: &str,
    keep: &[String],
) -> ApiResult<()> {
    let lbs = client.balancers().list(None).map_err(ApiError::from)?;
    for lb in lbs {
        if lb.meta.labels.get(label) == Some(&id.to_owned())
            && !keep.iter().any(|n| n == &lb.meta.name)
        {
            client.delete_balancer(&lb.meta.id).await?;
        }
    }
    Ok(())
}
