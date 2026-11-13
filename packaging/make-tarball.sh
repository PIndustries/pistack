#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 P. Industries LLC and PiStack contributors
#
# Pack one release tarball from built binaries.
#
#   packaging/make-tarball.sh BIN_DIR VERSION ARCH OUT_DIR
#   packaging/make-tarball.sh target/release 26.11.12.1 x86_64 dist
#
# Writes OUT_DIR/pistack-VERSION-linux-ARCH.tar.gz and a .sha256 next to it.
# VERSION only names the tarball. What the binaries print was fixed when they
# were built, by PISTACK_BUILD.
# The release workflow calls this; it also runs by hand.
set -euo pipefail

[ $# -eq 4 ] || { sed -n '5,13p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
bin_dir="$1"; version="$2"; arch="$3"; out_dir="$4"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
name="pistack-$version-linux-$arch"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/$name"

install -d "$stage/bin" "$stage/scripts" "$stage/systemd" "$stage/env"
install -m 0755 "$bin_dir/pistack" "$bin_dir/pi" "$stage/bin/"
install -m 0755 "$root/scripts/pistack-overlay.py" "$root/scripts/pistack-fip.py" \
  "$root/scripts/pistack-shelf-helper" "$root/scripts/pistack-youki-cleanup" "$stage/scripts/"
install -m 0644 "$root/scripts/pistack-fip-restore.service" "$stage/scripts/"
install -m 0644 "$root"/packaging/systemd/*.service "$stage/systemd/"
install -m 0644 "$root"/packaging/env/*.env "$stage/env/"
install -m 0755 "$root/packaging/install.sh" "$stage/install.sh"
install -m 0644 "$root/LICENSE" "$root/NOTICE" "$root/README.md" "$stage/"

mkdir -p "$out_dir"
out_dir="$(cd "$out_dir" && pwd)"
# Fixed owner and order, so the same inputs give the same listing.
if tar --version 2>/dev/null | grep -q 'GNU tar'; then
  tar -C "$work" --sort=name --owner=0 --group=0 --numeric-owner -czf "$out_dir/$name.tar.gz" "$name"
else
  tar -C "$work" -czf "$out_dir/$name.tar.gz" "$name"
fi
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$out_dir" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
else
  (cd "$out_dir" && shasum -a 256 "$name.tar.gz" > "$name.tar.gz.sha256")
fi
echo "$out_dir/$name.tar.gz"
