// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::sync::Arc;

use super::host_driver::HostDriver;

pub(crate) struct Entry {
    pub(crate) driver: Arc<dyn HostDriver>,
    pub(crate) local: bool,
}
