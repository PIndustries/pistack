// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Meta;
use serde::Deserialize;

use super::port_spec_view::PortSpecView;

#[derive(Deserialize)]
pub(crate) struct PortView {
    #[serde(flatten)]
    pub(crate) meta: Meta,
    pub(crate) spec: PortSpecView,
}
