# PiStack

A private cloud for VMs, containers and bare metal, written in Rust. One binary, one store, one API.

```text
pistack server --init          # first control-plane node
pistack agent  --join URL      # every host
pi machine create web --image ubuntu-24.04 --size m.2x4
```

PiStack treats OpenStack as the requirements list for what a private cloud must do, and Kubernetes as the requirements list for how containers should be orchestrated, then answers both with one system where a VM, a pod and a bare-metal box are peers: same scheduler, same network, same volumes, same identity, same DNS.

## Modules

Every module is a crate in `crates/` and a prefix on the one API port. Names follow a workshop theme; CLI nouns stay plain.

| Module | Owns | Replaces |
| --- | --- | --- |
| `gate` | identity, projects, roles, tokens, quotas, policy | Keystone, oslo.policy |
| `forge` | machines: VMs, containers, bare metal | Nova, Zun, Ironic, Masakari, Watcher, Blazar |
| `loom` | pods, deployments, services, ingress, controllers | Kubernetes control plane, Magnum |
| `anvil` | the per-host agent | nova-compute, neutron agents, os-brick, kubelet |
| `fabric` | networks, subnets, routers, security groups, floating IPs (OVN) | Neutron |
| `slab` | block volumes, snapshots, backups | Cinder |
| `mold` | images (VM and OCI) | Glance |
| `silo` | object storage, S3 | Swift |
| `shelf` | shared filesystems | Manila |
| `beacon` | DNS | Designate, CoreDNS |
| `relay` | load balancers, ingress | Octavia |
| `keep` | secrets, certificates, the cloud CA | Barbican |
| `blueprint` | declarative stacks, workflows, kits | Heat, Mistral, Helm |
| `gauge` | metrics, alarms, metering | Telemetry, CloudKitty |
| `bench` | the `pi` CLI and web console | Horizon, Skyline, openstackclient |

Shared crates: `pistack-types` (resource model, errors, the `Module` contract), `pistack-store` (embedded redb store behind a trait; Raft later), `pistack-policy`, `pistack-config`, `pistack-rpc`.

## Status

Scaffold. The binary boots, mounts every module, serves a discovery document at `/`. See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and [docs/ROADMAP.md](docs/ROADMAP.md).

## Building

```sh
cargo build
PISTACK_DATA_DIR=./data cargo run -- server --init --listen 127.0.0.1:8443 --insecure-http
curl localhost:8443/
```

## Licence

Apache-2.0; see [LICENSE](LICENSE). The reasoning is in [LICENSE-DECISION.md](LICENSE-DECISION.md). Contributions require a DCO sign-off (`git commit -s`). "PiStack" and the PiStack logo are trademarks of P. Industries LLC; the code is free, the name is not.
