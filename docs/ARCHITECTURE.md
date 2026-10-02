# Architecture

## Shape

One binary, two roles.

- `pistack server` — control plane. Opens the state store, mounts every module on one API port, runs reconcilers. Three of these for HA (Raft replication is the next step for `pistack-store`; today it is one node).
- `pistack agent` — a host. Runs `anvil`: VMs (libvirt/KVM or Cloud Hypervisor), pods (youki), bare metal (same binary as the provisioning ramdisk), OVN chassis, volume attach, metrics.

Everything a user touches enters the gateway (`bin/pistack/src/gateway.rs`) and is routed to a module by prefix. Modules talk to each other in-process and to hosts over gRPC (`pistack-rpc`). State changes publish `Event`s on a broadcast bus that reconcilers subscribe to.

## The resource model

Every object is a `Resource<Spec, Status>` (`pistack-types::resource`):

- `Meta` — id (`<kind>-<uuidv7>`), project, name, labels, annotations, timestamps, revision.
- `spec` — what the user asked for. Owned by the module.
- `status` — what is true. Written only by reconcilers.

This is deliberately the Kubernetes shape so `loom` can speak the k8s API later without translation, and so every module gets list/get/watch/revision-guarded update for free from `pistack-store::Collection<K>`.

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

`ModuleContext` carries the store, the event bus, a shutdown token, the public URL and the node name. Modules never bind ports or open files themselves.

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
| keep | Secret, Certificate, CertificateAuthority | software store, PKCS#11, Vault |
| blueprint | Stack, Workflow, Kit | every other module |
| gauge | Metric (Prometheus), Alarm, UsageRecord | anvil reports, events |
| bench | none | all APIs (CLI + console) |

## Scheduling

One scheduler in `forge` places every workload kind. It reads `Host` inventory (reported by anvil), subtracts allocations, applies filters (resources, affinity, labels, reservations from Blazar-style leases) and weighs the rest. `loom` pods become `Machine{kind: Container}` and go through the same path, so VMs and pods share capacity math.

## Networking

OVN-native, no plugin layer. `fabric` writes logical switches, routers, ports and ACLs to the OVN northbound DB; `anvil` runs the chassis side. Pods get logical switch ports exactly like VMs. DHCP and metadata are OVN features; no dnsmasq.

## Storage

`slab` owns volumes; `anvil` attaches them (LVM, Ceph RBD, NVMe-oF). Three reference backends in-tree, a plugin ABI for vendors. `mold` stores images content-addressed in `silo` (S3) or on local disk.

## Security

`keep` is the cloud CA. Every server and agent gets a certificate at join; every API call is mTLS or bearer-token over TLS. `gate` issues signed tokens (verifiable without a store lookup) and evaluates one policy document for the whole cloud.

## What is deliberately not here

RabbitMQ, a database per service, microversions, the Swift API, HOT templates, ML2, amphora VMs, Gnocchi, Django. See the design doc for why each was dropped.
