// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::shell_quote;
use super::write_executable;
use std::path::Path;

/// Stub `rbd`. `device list` and `showmapped` print the mapped image.
/// `device map` records it and prints `/dev/rbd0`. `device unmap` clears it.
pub(crate) fn write_rbd(path: &Path, log: &Path, state: &Path) {
    let log_q = shell_quote(log);
    let state_q = shell_quote(state);
    let body = format!(
        r#"#!/bin/sh
printf '%s\n' "$@" >> {log_q}
printf '\n' >> {log_q}
if [ "$1" = device ] && [ "$2" = list ]; then
  if [ -f {state_q} ]; then cat {state_q}; else printf '%s\n' '[]'; fi
  exit 0
fi
if [ "$1" = showmapped ]; then
  if [ -f {state_q} ]; then cat {state_q}; else printf '%s\n' '[]'; fi
  exit 0
fi
if [ "$1" = device ] && [ "$2" = map ]; then
  spec=$3
  pool=${{spec%%/*}}
  image=${{spec#*/}}
  printf '[{{"id":"0","pool":"%s","namespace":"","name":"%s","snap":"-","device":"/dev/rbd0"}}]\n' "$pool" "$image" > {state_q}
  printf '%s\n' /dev/rbd0
  exit 0
fi
if [ "$1" = device ] && [ "$2" = unmap ]; then
  printf '%s\n' '[]' > {state_q}
  exit 0
fi
echo unexpected >&2
exit 1
"#,
        log_q = log_q,
        state_q = state_q,
    );
    write_executable(path, &body);
}
