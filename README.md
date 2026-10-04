# PiStack

A private cloud for VMs, containers and bare metal, written in Rust. One binary, one store, one API.

```text
pistack server --init          # first control-plane node
pistack agent  --join URL      # every host
pi machine create web --image ubuntu-24.04 --size m.2x4
```

A virtual machine, a container, and a bare-metal machine are peers. They share one scheduler, one network, one set of volumes, one identity, and one DNS. A name, an address, and a running workload stay on the same records, in one binary.

## Modules

Every module is a crate in `crates/` and a prefix on the one API port. Names follow a workshop theme. CLI nouns stay plain.

| Module | Owns |
| --- | --- |
| `gate` | identity, projects, roles, tokens, quotas, policy |
| `forge` | machines: virtual machines, containers, bare metal |
| `loom` | pods, deployments, services, ingress, controllers |
| `anvil` | the agent on each host |
| `fabric` | networks, subnets, routers, security groups, floating IPs |
| `slab` | block volumes, snapshots, backups |
| `mold` | images for virtual machines and containers |
| `silo` | object storage, S3 |
| `shelf` | shared filesystems |
| `beacon` | DNS |
| `relay` | load balancers and ingress |
| `keep` | secrets, certificates, the cloud CA |
| `blueprint` | stacks, workflows, kits |
| `gauge` | metrics, alarms, metering |
| `bench` | the `pi` CLI and the web console |

Shared crates: `pistack-types` (resource model, errors, the `Module` contract), `pistack-store` (embedded redb store behind a trait; Raft later), `pistack-policy`, `pistack-config`, `pistack-rpc`.

## Status

The binary mounts every module on one API. The shape of the system is in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). The phase list is in [docs/ROADMAP.md](docs/ROADMAP.md).

## Building

```sh
cargo build
PISTACK_DATA_DIR=./data cargo run -- server --init --listen 127.0.0.1:8443 --insecure-http
curl localhost:8443/
```

## Licence

Apache-2.0; see [LICENSE](LICENSE). Contributions require a DCO sign-off (`git commit -s`). "PiStack" and the PiStack logo are trademarks of P. Industries LLC. The code is free. The name is not.
