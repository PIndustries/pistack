// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `validate_spec`: name, document, and parameters of a stack.

use pistack_types::ApiResult;

use super::stack_document::StackDocument;
use super::stack_spec::StackSpec;
use super::validate_document::validate_document;
use super::validate_resource_name::validate_resource_name;

/// Name, document, and parameters. Trims the name; leaves the document bytes
/// alone. Values are matched against the parameters, so a value for a
/// parameter that is not declared or not named is refused before the stack is
/// stored; a missing value is refused when the document is applied.
pub fn validate_spec(spec: &mut StackSpec) -> ApiResult<()> {
    spec.name = spec.name.trim().to_owned();
    validate_resource_name(&spec.name)?;
    validate_document(&spec.document)?;
    spec.normalize()
}
