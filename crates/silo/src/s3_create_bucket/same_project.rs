// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// A bucket belongs to the caller only when its project is exactly the
/// caller's. A cloud-scope bucket (no project) is not "ours" to reuse here:
/// an S3 caller always has a project, and reusing another scope's name would
/// hide a real `BucketAlreadyExists`.
pub(crate) fn same_project(
    existing: &crate::types::Bucket,
    project: Option<&pistack_types::Id>,
) -> bool {
    project.is_some() && existing.meta.project.as_ref() == project
}
