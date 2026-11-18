// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;
use relay::Client;

use crate::realizer::delete_unnamed::delete_unnamed;

pub(crate) async fn delete_labeled(client: &Client, label: &str, id: &str) -> ApiResult<()> {
    delete_unnamed(client, label, id, &[]).await
}
