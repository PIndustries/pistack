// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{ServiceStatusView, ServiceView};
use pistack_types::Resource;

/// Read-only service record. See [`ServiceView`].
pub type ServiceRecord = Resource<ServiceView, ServiceStatusView>;
