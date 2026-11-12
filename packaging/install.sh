#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 P. Industries LLC and PiStack contributors
#
# Install PiStack on this host: the pistack and pi binaries, the host helpers
# the server and agent call through sudo, the systemd units and their
# environment files.
#
#   sudo ./install.sh                  # everything
#   sudo ./install.sh --helpers-only   # only the helpers in /usr/local/sbin
#   sudo ./install.sh --no-units       # binaries and helpers, no systemd files
#   sudo ./install.sh --bin-dir DIR    # take pistack and pi from DIR
#
# Runs from an unpacked release tarball or from a checkout after
# `cargo build --release -p pistack -p bench`. Safe to re-run: binaries,
# helpers and units are replaced, files in /etc/pistack are left alone.
# Nothing is enabled or started. DESTDIR stages the files under another root.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -d "$here/bin" ]; then
  # Release tarball: everything sits next to this script.
  bin_dir="$here/bin"
  scripts_dir="$here/scripts"
else
  # Checkout: this script is packaging/install.sh.
  bin_dir="$here/../target/release"
  scripts_dir="$here/../scripts"
fi
units_dir="$here/systemd"
env_dir="$here/env"

helpers_only=0
units=1
while [ $# -gt 0 ]; do
  case "$1" in
    --helpers-only) helpers_only=1 ;;
    --no-units) units=0 ;;
    --bin-dir)
      [ $# -ge 2 ] || { echo "install.sh: --bin-dir needs a directory" >&2; exit 2; }
      bin_dir="$2"; shift ;;
    -h|--help) sed -n '5,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "install.sh: unknown option $1" >&2; exit 2 ;;
  esac
  shift
done

dest="${DESTDIR:-}"
if [ -z "$dest" ] && [ "$(id -u)" -ne 0 ]; then
  echo "install.sh: run as root, or set DESTDIR to stage the files" >&2
  exit 1
fi

say() { printf '==> %s\n' "$*"; }

# The Rust code calls the helpers by these exact paths, so the directory is
# not configurable. Each line is "file in scripts/" "installed name".
helpers=(
  "pistack-overlay.py pistack-overlay"
  "pistack-fip.py pistack-fip"
  "pistack-shelf-helper pistack-shelf-helper"
  "pistack-youki-cleanup pistack-youki-cleanup"
)

# Check everything is there before the first file is written.
missing=0
for h in "${helpers[@]}"; do
  read -r src _ <<< "$h"
  [ -f "$scripts_dir/$src" ] || { echo "install.sh: missing $scripts_dir/$src" >&2; missing=1; }
done
if [ "$helpers_only" -eq 0 ]; then
  for b in pistack pi; do
    [ -f "$bin_dir/$b" ] || { echo "install.sh: missing $bin_dir/$b" >&2; missing=1; }
  done
fi
[ "$missing" -eq 0 ] || exit 1

say "Helpers in $dest/usr/local/sbin"
install -d -m 0755 "$dest/usr/local/sbin"
for h in "${helpers[@]}"; do
  read -r src name <<< "$h"
  install -m 0755 "$scripts_dir/$src" "$dest/usr/local/sbin/$name"
done
if ! command -v python3 >/dev/null 2>&1; then
  echo "install.sh: warning: the helpers need python3 and it is not on this host" >&2
fi

if [ "$helpers_only" -eq 1 ]; then
  exit 0
fi

say "Binaries in $dest/usr/local/bin"
install -d -m 0755 "$dest/usr/local/bin"
install -m 0755 "$bin_dir/pistack" "$dest/usr/local/bin/pistack"
install -m 0755 "$bin_dir/pi" "$dest/usr/local/bin/pi"

if [ "$units" -eq 1 ]; then
  say "Units in $dest/etc/systemd/system"
  install -d -m 0755 "$dest/etc/systemd/system"
  install -m 0644 "$units_dir/pistack-server.service" "$dest/etc/systemd/system/pistack-server.service"
  install -m 0644 "$units_dir/pistack-agent.service" "$dest/etc/systemd/system/pistack-agent.service"
  install -m 0644 "$scripts_dir/pistack-fip-restore.service" "$dest/etc/systemd/system/pistack-fip-restore.service"

  # The environment files can hold the join token and the admin password,
  # so they are root-only, and an edited one is never replaced.
  install -d -m 0755 "$dest/etc/pistack"
  for role in server agent; do
    if [ -e "$dest/etc/pistack/$role.env" ]; then
      say "Keeping $dest/etc/pistack/$role.env"
    else
      install -m 0600 "$env_dir/$role.env" "$dest/etc/pistack/$role.env"
    fi
  done

  # /run/systemd/system exists only where systemd is the running init.
  if [ -z "$dest" ] && [ -d /run/systemd/system ]; then
    systemctl daemon-reload || true
  fi

  cat <<'NEXT'

Installed. Nothing is running yet.

  Control plane: set PISTACK_SERVER_ARGS in /etc/pistack/server.env
                 (--init on the first node, --join URL on the others), then
                   systemctl enable --now pistack-server
  Host agent:    set PISTACK_JOIN in /etc/pistack/agent.env, then
                   systemctl enable --now pistack-agent
NEXT
fi
