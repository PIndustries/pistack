// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A stack is a PiStack resource: a document the user wrote (YAML or JSON
//! text) plus declared parameters and the values given for them.
//!
//! A parameter is declared in `spec.declared` as a `name`, an optional
//! `default`, and an optional `description`. The document names one by
//! writing `{{ param.NAME }}` where a value belongs, quoted — bare `{{` is
//! the start of a mapping to YAML. Apply and update substitute it: the value
//! given in `spec.parameters` if there is one, else the declared default.
//! A reference stands for a whole value, so a number stays a number and a
//! flag a flag; set inside other text it is replaced within that text.
//!
//! An apply is refused before any row is written when a value was given for
//! a parameter nothing declares, when the document names a parameter that is
//! not declared, or when a declared parameter has neither a value nor a
//! default. Each message names the parameter.
//!
//! Apply creates network, subnet, machine, router, pod, listener, and record
//! set rows from the document, updates the rows an earlier apply of the same
//! stack created, and deletes managed rows the document no longer names. Each
//! successful stack apply records the spec, with the values it used, as a
//! revision; rollback puts an earlier revision back and applies it. A kit is a
//! stored document applied the same way. A workflow applies named stacks in
//! order and stops on the first error. Apply does not run a reconciler.

mod accepted_message;
mod applying_message;
mod kit_kind;
mod kit_spec;
mod parameter_decl;
mod revision_history_limit;
mod stack_document;
mod stack_phase;
mod stack_revision;
mod stack_spec;
mod stack_status;
mod template;
#[cfg(test)]
mod tests;
mod validate_document;
mod validate_kit;
mod validate_resource_name;
mod validate_spec;
mod validate_workflow;
mod workflow_accepted_message;
mod workflow_kind;
mod workflow_phase;
mod workflow_spec;
mod workflow_status;

pub use accepted_message::ACCEPTED_MESSAGE;
pub use applying_message::APPLYING_MESSAGE;
pub use kit_kind::KIT_KIND;
pub use kit_spec::Kit;
pub use kit_spec::KitSpec;
pub use kit_spec::KitStatus;
pub use parameter_decl::ParameterDecl;
pub use revision_history_limit::REVISION_HISTORY_LIMIT;
pub use stack_document::StackDocument;
pub use stack_phase::StackPhase;
pub use stack_revision::StackRevision;
pub use stack_spec::StackSpec;
pub use stack_status::Stack;
pub use stack_status::StackStatus;
pub use template::declared_uses;
pub use template::is_param_name;
pub use template::substitute;
pub use validate_document::validate_document;
pub use validate_kit::validate_kit;
pub use validate_spec::validate_spec;
pub use validate_workflow::validate_workflow;
pub use workflow_accepted_message::WORKFLOW_ACCEPTED_MESSAGE;
pub use workflow_kind::WORKFLOW_KIND;
pub use workflow_phase::WorkflowPhase;
pub use workflow_spec::WorkflowSpec;
pub use workflow_status::Workflow;
pub use workflow_status::WorkflowStatus;
