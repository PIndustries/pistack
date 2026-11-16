// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::podnet;

pub struct ObservedNet {
    pub(in crate::runtime::youki) addresses: Vec<String>,
    pub(in crate::runtime::youki) message: String,
    pub(in crate::runtime::youki) netns: String,
    pub(in crate::runtime::youki) veths: Vec<String>,
    pub(in crate::runtime::youki) ovs: Vec<podnet::OvsLink>,
}
