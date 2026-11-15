// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::youki) fn exec_argv(
    container_id: &str,
    argv: &[String],
    env: &[(String, String)],
    cwd: Option<&str>,
) -> Vec<String> {
    let mut args = Vec::with_capacity(4 + env.len() * 2 + argv.len());
    args.push("exec".into());
    if let Some(dir) = cwd {
        args.push("--cwd".into());
        args.push(dir.to_owned());
    }
    for (name, value) in env {
        args.push("--env".into());
        args.push(format!("{name}={value}"));
    }
    args.push(container_id.to_owned());
    args.push("--".into());
    args.extend(argv.iter().cloned());
    args
}
