#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 P. Industries LLC and PiStack contributors
#
# Boot a throwaway PiStack (server + agent) on this node and drive it with pi.
# Used by .github/workflows/lab.yml on the bare-metal runners.
set -euo pipefail
BIN="${1:-target/release}"
W=/tmp/pistack-lab; rm -rf "$W"; mkdir -p "$W"
PORT=18900
export PI_CONFIG="$W/pi.toml"
cleanup() { pkill -f "pistack --data-dir $W" || true; }
trap cleanup EXIT

PISTACK_ADMIN_PASSWORD=lab PISTACK_DNS_LISTEN=127.0.0.1:15399 \
  "$BIN/pistack" --data-dir "$W/server" server --init --listen 127.0.0.1:$PORT --insecure-http > "$W/server.log" 2>&1 &
sleep 2
# Real runtimes are used when present; mock otherwise (Phase 0).
"$BIN/pistack" --data-dir "$W/agent" agent --join http://127.0.0.1:$PORT > "$W/agent.log" 2>&1 &
sleep 3

"$BIN/pi" login http://127.0.0.1:$PORT --name admin --password-stdin <<< lab
"$BIN/pi" host list
"$BIN/pi" network create lan --kind overlay
"$BIN/pi" subnet create lan4 --network lan --cidr 10.42.0.0/24 --gateway 10.42.0.1
timeout 60 "$BIN/pi" machine create smoke --size s.1x1 --image ubuntu --network lan --wait
"$BIN/pi" machine list -o wide
"$BIN/pi" machine delete smoke --yes --wait || "$BIN/pi" machine delete smoke --yes
echo "lab smoke: OK on $(hostname)"
