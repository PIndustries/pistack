// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `validate_kit`: name, document, and parameters of a kit.

use pistack_types::ApiResult;

use super::kit_spec::KitSpec;
use super::stack_document::StackDocument;
use super::validate_document::validate_document;
use super::validate_resource_name::validate_resource_name;

/// Name, document, and parameters. Same rules as a stack.
pub fn validate_kit(spec: &mut KitSpec) -> ApiResult<()> {
    spec.name = spec.name.trim().to_owned();
    validate_resource_name(&spec.name)?;
    validate_document(&spec.document)?;
    spec.normalize()
}
