// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

use super::floating_ip_spec_view::FloatingIpSpecView;

#[derive(Deserialize)]
pub(crate) struct FloatingIpView {
    pub(crate) spec: FloatingIpSpecView,
}
