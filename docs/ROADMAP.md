# Roadmap

Each phase ends at a gate an outside tool decides.

## Phase 0 — Foundation (now)

- [x] Cargo workspace, `pistack` binary, gateway, discovery document
- [x] `pistack-types`: resource model, errors, Module contract
- [x] `pistack-store`: redb-backed typed collections with name index, revisions, events
- [ ] `pistack-policy`: rule language + evaluator
- [ ] `pistack-config`: TOML + env + flags
- [ ] `pistack-rpc`: host RPC (gRPC) and event bus
- [ ] `gate`: projects, principals, roles, tokens, quotas; auth layer on the gateway
- [ ] `keep`: cloud CA, TLS for the gateway and agents, secrets CRUD
- [ ] `forge` + `anvil`: Host registration, Machine CRUD, scheduler, a `mock` driver that "runs" machines
- [ ] `fabric`: Network/Subnet/Port/SecurityGroup model with a mock backend
- [ ] `slab` + `mold`: Volume and Image model with local-file backends
- [ ] `bench`: `pi` CLI covering the above
- Gate: `pistack server --init` + `pistack agent --join` on one box; `pi machine create` reaches Ready on the mock driver; integration test in CI.

## Phase 1 — Bootable cloud

- libvirt/KVM driver in anvil; cloud-init metadata
- OVN backend in fabric; DHCP, metadata, floating IPs, security groups
- LVM and Ceph RBD in slab; qemu-img in mold
- TLS everywhere via keep
- Gate: a VM boots on three nodes and gets an IP, a volume and a floating IP from the CLI.

## Phase 2 — Containers and platform

- loom: pods, deployments, services, ingress on youki; k8s API compatibility decision
- relay (OVN L4 + Pingora L7), beacon (hickory), silo (S3), gauge (Prometheus)
- Gate: a stack with a VM database and a pod web tier behind one relay, one DNS name.

## Phase 3 — Everything else

- shelf, blueprint kits, Cloud Hypervisor driver, bare-metal provisioning, openstack-compat shim, web console, Raft replication of the store.
