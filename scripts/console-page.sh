#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 P. Industries LLC and PiStack contributors
#
# Print the path to the assembled console page, so you can open it in a
# browser. The console lives in small files under crates/bench/src/console/
# and is joined into one page at build time; this builds it if needed and
# tells you where it came out.
#
#   ./scripts/console-page.sh
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p bench >/dev/null 2>&1 || cargo build -p bench
out=$(find target -path '*/out/console.html' -print -quit)
if [ -z "$out" ]; then
  echo "no assembled console page found; run cargo build -p bench" >&2
  exit 1
fi
printf '%s\n' "$PWD/$out"
