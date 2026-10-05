# Architecture

## Shape

One binary, two roles.

- `pistack server` — control plane. Opens the state store, mounts every module on one API port, runs reconcilers. One process is already a one-voter cluster and announces itself on its private address. A new process with an empty data directory discovers that cluster and adopts it, or `--join URL` names it. `--init` founds a cluster even if peers answer. An existing store never joins away. The second server replicates as a learner. The third makes all three voters. Later servers stay learners until promoted. Writes are accepted by the leader. Controllers and reconcile loops run only on the current leader; followers stay idle and apply the replicated log. A learner does not become leader.
- `pistack agent` — a host. Runs `anvil`: VMs (libvirt/KVM or Cloud Hypervisor), pods (youki), bare metal (same binary as the provisioning ramdisk), OVN chassis, volume attach, metrics.

Everything a user touches enters the gateway (`bin/pistack/src/gateway.rs`) and is routed to a module by prefix. Modules talk to each other in-process and to hosts through `pistack-rpc`, a typed command queue with JSON-over-HTTP long-poll today (agents call in; no inbound ports on hosts). State changes publish `Event`s on an in-process broadcast bus that reconcilers subscribe to.

### Why not NATS or gRPC everywhere

The NATS server is Go and there is no production embeddable NATS server in Rust (every Rust crate is a client), so "NATS embedded" would mean shipping a Go binary inside PiStack. gRPC (tonic) is pure Rust but point-to-point, not a bus. So the transport sits behind a trait: the embedded default is pure Rust (command queue + event bus, persisted by the store), `async-nats` is a planned pluggable transport for sites that already run NATS, and a small Rust server speaking the core NATS wire protocol is a Phase 3 option if we want those semantics embedded.

## The resource model

Every object is a `Resource<Spec, Status>` (`pistack-types::resource`):

- `Meta` — id (`<kind>-<uuidv7>`), project, name, labels, annotations, timestamps, revision.
- `spec` — what the user asked for. Owned by the module.
- `status` — what is true. Written only by reconcilers.

Every module uses that shape, so list, get, watch, and revision-guarded update come from `pistack-store::Collection<K>`. `loom` stores pods and deployments as the same kind of record.

Names are unique per project per kind. `Collection::resolve` accepts a name or an id, which is what lets the CLI say `pi machine show web`.

## The Module contract

```rust
#[async_trait]
pub trait Module {
    fn info(&self) -> ModuleInfo;              // name, prefix, summary
    fn router(&self, ctx: ModuleContext) -> Router;  // mounted at /{prefix}
    async fn start(&self, ctx: ModuleContext) -> Result<()>; // reconcilers
}
```

`ModuleContext` carries the store, the event bus, a shutdown token, the public URL, the node name, and cluster leadership. Controllers watch leadership and stay idle on followers. Modules never bind ports or open files themselves (beacon's DNS listener is the documented exception). DNS listen rules: when `PISTACK_DNS_LISTEN` is set it is honored exactly as a single `host:port` or a comma-separated multi-bind list; when unset, beacon binds the server's private/cluster listen address (the same private unicast used for advertise) on port `5353`; when no private address is known it falls back to `127.0.0.1:5353`. Defaults never use `0.0.0.0`. The node name is the one stable identity a machine has: the agent enrolls, says hello, heartbeats, and polls under it; forge keys the host record by it; loom stamps it as `pistack.io/hostname`; the server passes it to modules. It is picked once and kept in `<data_dir>/node.name`, so a restart, a host rename, or a re-enroll keeps it. Order: `PISTACK_NODE_NAME` (written back to the file, so it is also how to rename a node on purpose), then `node.name`, then the common name of a certificate the agent already enrolled with, then the host name, then a minted `node-<hex>`. A server and an agent sharing a data dir share the name. On `/rpc`, a client certificate speaks only for the node named in it (403 otherwise); an agent whose saved certificate names another node re-enrolls when it has a join token and refuses to start when it does not. `<data_dir>/node.id` is the separate raft member id for control-plane membership.

## Who owns what

| Module | Resources | Reconciles against |
| --- | --- | --- |
| gate | Project, Principal, Role, RoleBinding, Token, Quota, Policy | nothing (pure state) |
| forge | Machine, Host, Flavor (size), Reservation | anvil over RPC |
| loom | Pod, Deployment, ReplicaSet, Service, Ingress, Job, CronJob, ConfigMap | forge (pods are machines of kind container) |
| anvil | none in the store; reports Host status | libvirt, youki, OVN, LVM/Ceph |
| fabric | Network, Subnet, Port, Router, SecurityGroup, FloatingIp | OVN northbound DB |
| slab | Volume, Snapshot, Backup, VolumeType | storage backends via anvil |
| mold | Image, ImageMember | silo or local store |
| silo | Bucket, Object (S3 API) | local disks |
| shelf | Share, ShareAccess | CephFS / NFS |
| beacon | Zone, RecordSet | its own authoritative server (hickory) |
| relay | Balancer, Listener, Pool, Member, HealthMonitor | OVN LB (L4), Pingora (L7) |
| keep | Secret, Certificate, CertificateAuthority | root key: software store (default), Vault Transit (partial), PKCS#11 (parked) |
| blueprint | Stack, Workflow, Kit | every other module |
| gauge | Metric (Prometheus), Alarm, UsageRecord | anvil reports, events |
| bench | none | all APIs (CLI + console) |

## Scheduling

One scheduler in `forge` places every workload. It reads `Host` inventory reported by anvil, subtracts allocations, applies filters (resources, affinity, labels, reservations) and weighs the rest. A `loom` pod becomes `Machine{kind: Container}` and takes the same path, so a virtual machine and a pod share capacity math.

A pod's liveness and readiness probes run on the host agent. `http_get` and `tcp_socket` connect to the sandbox address. `exec` runs its command in the named container through the same youki exec path as `pi pod exec`, so `PISTACK_YOUKI_SUDO` and the rootless fallback apply unchanged. Exit code 0 is a pass. A non-zero exit or a timeout is a failure. The Ready condition waits until each readiness streak reaches `success_threshold`. A liveness streak that reaches `failure_threshold` restarts that container when the restart policy allows it. A runtime other than youki reports an exec probe unsupported. That report is not a pass and does not count as a failure. The excerpt kept from the command is short, and environment values are not logged.

## Networking

`fabric` owns networks, subnets, ports, routers, security groups, and floating IPs. A project network is an overlay segment: its own bridge, its own VXLAN id, and its own router namespace, so two projects can use the same CIDR without seeing each other. Hosts share an underlay they report when they join. Guests on one network reach each other through a static forwarding table on that segment. The project router connects the network to an external network. When that external network is a provider segment or an overlay segment, the floating address is installed on the host that owns the port. A provider network is a bridge on a named physical link, with a VLAN when one is set. DHCP from the project router hands the guest the address allocated for its port, plus the subnet DNS servers and host routes. Security groups are installed with that plan, and metadata is answered on the host that holds the tap. The port records the host that runs the guest. The scheduler places a machine that names a network only on a host that has reported an underlay address. `PISTACK_FABRIC_BACKEND=overlay` selects this datapath. `PISTACK_FABRIC_BACKEND=hostnat` remains the single-host datapath: the address on the port is the address inside the guest. Virtual machines and pods use the same network records.

## Storage

`slab` owns volumes; `anvil` attaches them (LVM, Ceph RBD, NVMe-oF). Three reference backends in-tree, a plugin ABI for vendors. `mold` stores images content-addressed in `silo` (S3) or on local disk.

## Security

`keep` is the cloud CA. Every server and agent gets a certificate at join; every API call is mTLS or bearer-token over TLS. `gate` issues signed tokens (verifiable without a store lookup) and evaluates one policy document for the whole cloud.

## One system

Modules run in the `pistack` process and share one store. Agents call in over `pistack-rpc`. Stacks are PiStack YAML applied with `pi apply`. `gate` evaluates one policy document for the whole cloud. `keep` is the cloud CA.
