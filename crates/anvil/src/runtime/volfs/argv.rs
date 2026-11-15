// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
pub(in crate::runtime::volfs) fn argv(program: &str, args: &[String]) -> Vec<String> {
    let mut step = Vec::with_capacity(args.len() + 1);
    step.push(program.to_owned());
    step.extend(args.iter().cloned());
    step
}
