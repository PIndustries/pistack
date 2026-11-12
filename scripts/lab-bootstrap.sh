#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Copyright (c) 2026 P. Industries LLC and PiStack contributors
#
# Prepare a bare-metal Ubuntu box as a PiStack lab node and (optionally)
# register it as a GitHub Actions self-hosted runner for pindustries/pistack.
#
#   sudo ./scripts/lab-bootstrap.sh --report              # just print what this box has
#   sudo ./scripts/lab-bootstrap.sh --install             # install KVM, libvirt, OVS/OVN, LVM, Rust
#   sudo ./scripts/lab-bootstrap.sh --helpers             # put the host helpers in /usr/local/sbin
#   sudo ./scripts/lab-bootstrap.sh --runner TOKEN [--labels kvm,ovn,gpu]
#        (TOKEN from: gh api -X POST repos/PIndustries/pistack/actions/runners/registration-token -q .token)
#
# Safe to re-run. Touches nothing outside packages, /opt/actions-runner, the
# helpers in /usr/local/sbin and a dedicated `pistack` user.
set -euo pipefail

REPO_URL="https://github.com/PIndustries/pistack"
RUNNER_VERSION="${RUNNER_VERSION:-2.328.0}"
LABELS="pistack-lab"

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }

report() {
  say "Host"
  echo "hostname:   $(hostname)"
  echo "os:         $(. /etc/os-release && echo "$PRETTY_NAME")"
  echo "kernel:     $(uname -r)"
  echo "arch:       $(uname -m)"
  echo "cpus:       $(nproc)"
  echo "memory:     $(awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo)"
  echo "root disk:  $(df -h / | awk 'NR==2 {print $4 " free of " $2}')"
  say "Virtualisation"
  if [ -e /dev/kvm ]; then echo "/dev/kvm:   present"; else echo "/dev/kvm:   MISSING (enable VT-x/AMD-V or nested virt)"; fi
  grep -qE 'vmx|svm' /proc/cpuinfo && echo "cpu flags:  vmx/svm ok" || echo "cpu flags:  no vmx/svm (ARM hosts use KVM via EL2; check /dev/kvm)"
  for b in virsh qemu-system-x86_64 qemu-system-aarch64 qemu-img ovs-vsctl ovn-nbctl lvcreate rbd youki cargo nvidia-smi; do
    printf '%-20s %s\n' "$b:" "$(command -v $b >/dev/null && echo yes || echo no)"
  done
  say "Network"
  ip -br link | grep -vE '^(lo|veth|docker|virbr|br-)' || true
  ip -br addr | grep -vE '^(lo|veth|docker|virbr|br-)' || true
  say "Block devices (unmounted disks could back LVM/Ceph)"
  lsblk -dno NAME,SIZE,TYPE,MOUNTPOINT | grep -v loop || true
  command -v nvidia-smi >/dev/null && { say "GPUs"; nvidia-smi -L; } || true
}

install() {
  say "Installing KVM, libvirt, OVS/OVN, LVM, build tools"
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq qemu-kvm qemu-utils libvirt-daemon-system libvirt-clients libvirt-dev \
    openvswitch-switch ovn-central ovn-host lvm2 thin-provisioning-tools \
    build-essential pkg-config libssl-dev clang protobuf-compiler jq curl git socat cloud-image-utils
  systemctl enable --now libvirtd openvswitch-switch ovn-central ovn-controller >/dev/null 2>&1 || true
  id pistack >/dev/null 2>&1 || useradd -m -s /bin/bash -G libvirt,kvm pistack
  usermod -aG libvirt,kvm pistack
  if ! sudo -u pistack bash -lc 'command -v cargo' >/dev/null 2>&1; then
    say "Installing Rust for user pistack"
    sudo -u pistack bash -lc 'curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c clippy,rustfmt'
  fi
  # OVN in single-node mode so fabric can talk to the NB DB on this box.
  ovs-vsctl set open . external-ids:ovn-remote=unix:/var/run/ovn/ovnsb_db.sock \
    external-ids:ovn-encap-type=geneve external-ids:ovn-encap-ip="$(hostname -I | awk '{print $1}')" \
    external-ids:system-id="$(hostname)" || true
  helpers
  say "Done"
  report
}

helpers() {
  say "Installing the host helpers"
  "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../packaging/install.sh" --helpers-only
}

runner() {
  local token="$1"
  [ -n "$token" ] || { echo "usage: --runner TOKEN"; exit 2; }
  [ -e /dev/kvm ] && LABELS="$LABELS,kvm"
  command -v ovn-nbctl >/dev/null && LABELS="$LABELS,ovn"
  command -v nvidia-smi >/dev/null && LABELS="$LABELS,gpu"
  [ -n "${EXTRA_LABELS:-}" ] && LABELS="$LABELS,$EXTRA_LABELS"
  local arch; case "$(uname -m)" in x86_64) arch=x64;; aarch64) arch=arm64;; *) echo "unsupported arch"; exit 1;; esac
  say "Registering GitHub runner $(hostname) with labels $LABELS"
  mkdir -p /opt/actions-runner && chown pistack: /opt/actions-runner
  cd /opt/actions-runner
  if [ ! -x ./config.sh ]; then
    sudo -u pistack curl -sSLo runner.tgz "https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-linux-${arch}-${RUNNER_VERSION}.tar.gz"
    sudo -u pistack tar xzf runner.tgz && rm runner.tgz
  fi
  sudo -u pistack ./config.sh --unattended --replace --url "$REPO_URL" --token "$token" \
    --name "$(hostname)" --labels "$LABELS" --work _work
  ./svc.sh install pistack && ./svc.sh start
  say "Runner online. It only runs jobs from $REPO_URL (private repo)."
}

case "${1:-}" in
  --report) report ;;
  --install) install ;;
  --helpers) helpers ;;
  --runner) tok="${2:-}"; [ "${3:-}" = "--labels" ] && EXTRA_LABELS="${4:-}"; runner "$tok" ;;
  *) sed -n '5,16p' "$0"; exit 2 ;;
esac
