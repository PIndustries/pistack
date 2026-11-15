// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// A command probe needs at least one argument, and no argument may contain
/// a NUL byte. youki would reject the same argv; checking it here keeps the
/// probe message stable.
pub(crate) fn validate_exec_argv(argv: &[String]) -> Result<(), &'static str> {
    if argv.is_empty() || argv.iter().any(|arg| arg.is_empty()) {
        return Err("exec command is empty");
    }
    if argv.iter().any(|arg| arg.contains('\0')) {
        return Err("exec command contains a NUL byte");
    }
    Ok(())
}
