// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::doc_item::doc_item;
use super::doc_item_type::DocItem;
use super::parse_all::parse_all;

pub(crate) fn parse_document(document: &str) -> ApiResult<Vec<DocItem>> {
    parse_all(document)?
        .into_iter()
        .filter(|value| !value.is_null())
        .map(doc_item)
        .collect()
}
