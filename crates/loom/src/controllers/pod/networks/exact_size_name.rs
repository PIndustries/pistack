// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use pistack_types::Resources;
/// Name of the auto-created size for a pod that fits no existing one.
pub fn exact_size_name(r: &Resources) -> String {
    format!("pod-{}x{}", r.vcpus.max(1), r.memory_mib.max(1))
}
