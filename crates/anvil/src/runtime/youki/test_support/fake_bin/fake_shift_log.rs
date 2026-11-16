// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Skip youki's global `--log <file>` so the subcommand is `$1`.
pub(crate) fn fake_shift_log() -> &'static str {
    r#"
cmd=$1
if [ "$1" = --log ]; then
  cmd=$3
fi
"#
}
