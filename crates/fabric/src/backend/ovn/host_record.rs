// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::{host_status_view::HostStatusView, host_view::HostView};
use pistack_types::Resource;

/// Read-only host record. See [`HostView`].
pub type HostRecord = Resource<HostView, HostStatusView>;
