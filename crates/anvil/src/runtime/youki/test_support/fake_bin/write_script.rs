// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::shell_quote;
use super::write_exe;
use std::path::Path;

/// Fake youki. `state` reports a container only after `create` (or when
/// the test pre-seeds `youki-ids/<id>`). Missing containers say
/// "does not exist" so a pre-create check does not delete them.
pub(crate) fn write_script(path: &Path, log: &Path, exit_code: i32) {
    let quoted = shell_quote(log);
    let track = shell_quote(&log.parent().unwrap().join("youki-ids"));
    let stderr = if exit_code == 0 {
        String::new()
    } else {
        "echo 'youki exploded' >&2\n".to_owned()
    };
    let body = format!(
        r#"#!/bin/sh
printf '%s\n' "$@" >> {quoted}
printf '\n' >> {quoted}
mkdir -p {track}
cmd=
id=
skip=0
for arg in "$@"; do
  if [ "$skip" = 1 ]; then
skip=0
continue
  fi
  case "$arg" in
--log|--bundle|--cwd|--env)
  skip=1
  ;;
--*)
  ;;
*)
  if [ -z "$cmd" ]; then
    cmd=$arg
  fi
  id=$arg
  ;;
  esac
done
if [ "$cmd" = state ]; then
  if [ -f {track}/"$id" ]; then
printf '%s\n' '{{"pid":4242,"status":"running"}}'
exit 0
  fi
  echo "container $id does not exist" >&2
  exit 1
fi
if [ "$cmd" = create ]; then
  : > {track}/"$id"
fi
if [ "$cmd" = delete ]; then
  rm -f {track}/"$id"
fi
{stderr}exit {exit_code}
"#
    );
    write_exe(path, &body);
}
