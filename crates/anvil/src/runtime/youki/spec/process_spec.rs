// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::DEFAULT_PATH;
use crate::runtime::youki::image::ImageConfig;
use crate::runtime::youki::spec::Launch;

pub(in crate::runtime::youki) fn process_spec(
    launch: Option<&Launch>,
    cfg: &ImageConfig,
) -> (Vec<String>, Vec<String>, String) {
    let (cmd, args) = match launch {
        Some(l) if !l.command.is_empty() && !l.args.is_empty() => {
            (l.command.clone(), l.args.clone())
        }
        Some(l) if !l.command.is_empty() => (l.command.clone(), Vec::new()),
        Some(l) if !l.args.is_empty() => (cfg.entrypoint.clone(), l.args.clone()),
        _ => (cfg.entrypoint.clone(), cfg.cmd.clone()),
    };
    let mut proc = cmd;
    proc.extend(args);
    if proc.is_empty() {
        proc.push("/bin/sh".into());
    }
    let mut env_map = std::collections::BTreeMap::new();
    env_map.insert("PATH".into(), DEFAULT_PATH.into());
    for entry in &cfg.env {
        if let Some((k, v)) = entry.split_once('=') {
            if !k.is_empty() {
                env_map.insert(k.to_owned(), v.to_owned());
            }
        }
    }
    if let Some(l) = launch {
        for (k, v) in &l.env {
            env_map.insert(k.clone(), v.clone());
        }
    }
    let env = env_map
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    let cwd = launch
        .filter(|l| !l.cwd.is_empty())
        .map(|l| l.cwd.clone())
        .or_else(|| {
            if cfg.working_dir.is_empty() {
                None
            } else {
                Some(cfg.working_dir.clone())
            }
        })
        .unwrap_or_else(|| "/".into());
    (proc, env, cwd)
}
