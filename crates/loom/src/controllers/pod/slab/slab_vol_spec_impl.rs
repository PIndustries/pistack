// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::slab::SlabVolSpec;
use pistack_types::Spec;
impl Spec for SlabVolSpec {
    const KIND: &'static str = pistack_types::kind::VOLUME;
}
