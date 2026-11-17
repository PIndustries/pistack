// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `WORKFLOW_ACCEPTED_MESSAGE`: status text stored when a workflow is created.
/// Status text stored when a workflow is created. Apply replaces it with
/// the stack names it ran.
pub const WORKFLOW_ACCEPTED_MESSAGE: &str =
    "accepted; apply runs each named stack in order and stops on the first error";
