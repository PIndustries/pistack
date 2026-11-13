# Status

What a private cloud needs, area by area, and where PiStack stands on each.

Read from the code on main at 7f39751, 2026-11-12. Each line is one of:

- **Done**: in the tree and reachable through the API.
- **Partial**: some of it works; the line says what does not.
- **In progress**: in an open pull request.
- **Missing**: no code.

"No CLI" means it works over HTTP or `pi api` but `pi` has no command for it.

## Bugs found while reading

All fixed on main:

- #113: a project admin was a cloud admin; a disabled user's token and a deleted key's token kept working; a revoked agent certificate was accepted; the first admin password was logged; CORS was permissive.
- #114: the TCP load balancer used only the first member; on hostnat a port in several security groups got only the last group's rules.
- #115: clone and volume-from-snapshot gave a blank volume on LVM and RBD.
- #116: a machine could boot any project's image; a cloud-hypervisor guest wrote into the shared image and got no cloud-init. `pi machine create --volume` is refused with a message instead of ignored; attaching at start is still to do.
- #117: `/cluster`, including the full store snapshot, was open by default. It now needs a member or a join token.

Found by the three-server lab run, fixes in open pull requests or in progress:

- #119: a failed network or load balancer reconcile was retried without pause, and a follower that fell behind a busy leader never caught up, after which no write was confirmed.
- Hosts cannot follow a leader change: they are sent to the servers' plain HTTP address, where they cannot show their certificate (in progress).
- An admin cannot read cluster status since #117 (in progress).
- A server that led and had writes no majority confirmed has to be wiped and rejoined by hand when it returns.
- Every machine is given a floating IP as soon as an external network exists. There is no switch for it.

## Compute

Done
- Create, delete, start, stop, reboot
- Sizes (flavors)
- Quota per project on machines, vCPUs, RAM and root disk
- cloud-init user data and SSH keys on libvirt
- Metadata service, EC2 layout, on overlay and hostnat
- Live migration on libvirt (no CLI)
- Scheduler: host filters, label selector, spread weights
- Per-host resource accounting; a host that misses heartbeats is marked down
- Volume hot-plug on libvirt
- Host labels set by an admin; the `zone` label is the availability zone a machine can ask for
- Disable and enable a host
- Runtimes: libvirt/KVM, cloud-hypervisor, youki containers, bare metal power, mock
- Pods, Deployments, StatefulSets, DaemonSets, Jobs, CronJobs, Services, Ingress, Autoscaler (loom)

Partial
- SSH keys: raw strings per machine, no keypair resource
- VNC console: only when the VM is on the API server's own host; libvirt only; no serial console
- Console log: written on the host, no API to read it for a VM
- Affinity: soft hints by machine name; no server groups, no hard rule
- Host aggregates: labels and a host selector only; no aggregate resource
- Drain: live-migrates running virtual machines on libvirt hosts; everything else is reported and left
- Evacuate off a dead host: machines start elsewhere from their image without the root disk; RBD volumes follow, local ones do not; no fencing
- Compute quota: no GPU limit; a pod's machine is counted but a pod is not refused
- NIC hot-plug: the agent can do it, nothing on the server asks it to
- Boot from volume: the flag is stored, no runtime boots from one
- Root disk: one copy-on-write disk on libvirt; no extra ephemeral or swap disks
- GPU: in sizes and the scheduler; hosts always report zero and there is no passthrough
- Bare metal: power and PXE boot; does not write an image, no inspection or enrollment
- Metadata service on OVN: no path from the guest

Missing
- Resize
- Cold migration
- Shelve, rescue, rebuild
- Snapshot a machine to an image
- Pause and suspend
- Overcommit ratios; the pack strategy exists but cannot be switched on
- PCI passthrough, SR-IOV, NUMA, hugepages, CPU pinning
- UEFI, Secure Boot, vTPM

## Images

Done
- Upload, download, import from URL (raw, qcow2, oci)
- qcow2 safety checks; URL import blocks internal addresses
- Private, shared with a project, or public (sharing has no CLI)
- sha256 checked on every host download
- Quota per project on image count and stored bytes; the bytes are checked once an upload is in

Partial
- Minimum disk and RAM are stored and never checked
- Host image cache has no cleanup and no resume
- Storing images in object storage: written, not switched on (In progress, PR 110)

Missing
- Image signing
- Image from volume

## Network

Backends: OVN, overlay (VXLAN and namespaces), hostnat (one host), mock.

Done
- Networks, subnets, allocation pools, ports, fixed IPs, MACs
- Routers, external gateway, SNAT (IPv4)
- Floating IPs on OVN, overlay and hostnat
- Security groups: stateful, ingress and egress, remote group, default group per project
- Port security toggle
- Provider networks, flat and VLAN
- Geneve or VXLAN on OVN; VXLAN on overlay
- DHCP on OVN and overlay
- Quota per project on networks, subnets, routers, security groups, ports and floating IPs
- OVN gateway HA across chassis; distributed floating IPs

Partial
- IPv6: addresses and rules work; no router advertisements on OVN, SNAT is IPv4 only, hostnat ignores it
- MTU: accepted, not reconciled with tunnel overhead
- Static routes: OVN only; no CLI
- Instance DNS: only through a labelled zone; guests are not pointed at it
- Shared networks: cloud-wide or private, nothing between
- Overlay routers live on one host with no failover
- CLI has no `update` for any network resource

Missing
- Allowed address pairs
- Trunk ports
- QoS and bandwidth limits
- VPN, firewall policies, BGP, port mirroring
- Per-project network sharing (RBAC)
- Topology and IP availability views

Lab-proven on two hosts (OVN): networks, DHCP, traffic between hosts, a router with an external gateway, floating IPs in and out on both hosts, outbound NAT, security groups denying by default and allowing by rule, a flat provider network. Not yet run on hosts: VLAN provider networks, IPv6, gateway failover between hosts.

## Load balancers

Done
- Balancers, listeners, pools, members
- TCP, UDP, HTTP with host and path routes
- TLS termination from a stored certificate
- TCP and HTTP health checks

Partial
- Runs inside the server process on the server's address; the VIP is never placed on a tenant network
- Algorithm, session persistence and weights are stored and ignored
- Connection limit works on HTTP only
- Floating IP field is unused
- No CLI

Missing
- OVN load balancer (stub)
- A data plane off the control-plane host
- Header, cookie and redirect rules; backend re-encryption

## DNS

Done
- Zones and record sets, primary and secondary
- A, AAAA, CNAME, MX, TXT, SRV, NS, PTR, wildcards
- Authoritative server over UDP and TCP
- Outbound AXFR and secondary pull
- Automatic records for machines and floating IPs

Partial
- Reverse DNS by hand only
- No CLI

Missing
- NOTIFY, IXFR, TSIG, DNSSEC

## Block storage

Done
- Create, delete, attach, detach
- Snapshots, clone and volume from snapshot on local, LVM and RBD
- Volume from image
- Volume types
- Quota per project on volume count and gigabytes, snapshots, and backup count and gigabytes; extend is checked
- Backup to object storage and restore (no CLI)
- Backends: local file, LVM thin, Ceph RBD

Partial
- Extend works; an attached guest is not told the disk grew
- LVM and RBD have only been run against stand-in scripts, never real Ceph or LVM

In progress (PR 109)
- A local or LVM volume stays on its node; machines are placed with their volume
- RBD backup restore

Missing
- QoS
- Encryption at rest (flag stored, never read)
- Multi-attach
- Migration and retype
- Transfer between projects
- Consistency groups
- Placement by free capacity
- iSCSI, NVMe, NFS backends

## Object storage

Done
- Buckets and objects
- S3 path-style: get, put, head, delete, copy, list v2, multipart
- SigV4, presigned URLs, streaming uploads
- Cleanup of abandoned multipart uploads

Partial
- S3 has no bucket calls, batch delete, range GET, list v1, virtual-host style or tagging
- Versioning keeps old bytes but cannot list or fetch them
- Quota: bucket count per project only; no limit on bytes
- No CLI

In progress (PR 110)
- Object bytes copied between control-plane servers (full copies)

Missing
- ACLs and bucket policies
- Lifecycle and expiry
- Erasure coding
- Static websites

## Shared filesystems

Done
- NFS shares with access by client CIDR

Partial
- CephFS share is created; access rules are stored and not applied
- NFS size is not enforced
- No CLI

Missing
- Share snapshots, types, networks, resize

## Identity

Done
- Users, service accounts, projects, roles, role bindings
- JWT tokens with expiry; API keys
- argon2id passwords
- Quota framework: limits per project, set by an admin, read with usage by members (`pi quota`)
- Audit log: every change, login, secret read and refusal, with principal, API key, client address and outcome; read by a cloud admin with filters and paging (`pi audit list`); kept 90 days and 50,000 records by default

Partial
- Roles: admin, member, reader only; custom roles grant nothing; the policy engine is built and unused
- Nested projects: parent recorded, nothing inherited
- Audit log: admin only, no per-project view; ordinary reads, object uploads and server joins are not recorded; no export or forwarding to another system
- Password rule is "not empty"; no lockout

Missing
- Signing-key rotation; a single token cannot be revoked, only its user disabled or its key deleted
- Groups, domains
- OIDC, SAML, LDAP, federation
- MFA

## Secrets

Done
- Encrypted secrets with versions
- Cloud CA, certificates, revocation list
- Root key in software or wrapped by Vault Transit
- S3 access keys; join tokens with limits

Partial
- No re-encryption under a new root key; `rotate_after_days` is ignored
- Agent certificates are never renewed
- Vault tested against a mock and a dev server only
- Secrets and certificates have no CLI

Missing
- PKCS#11

## Orchestration

Done
- Stacks, kits, workflows
- Apply, update, history, rollback

Partial
- Parameters are stored and never substituted
- Apply does not wait for resources to be ready
- Covers network, subnet, router, listener, DNS and machine only

Missing
- Drift detection

## Telemetry

Done
- Host usage from the agent: load, memory used, free space on the data directory (`pi host stats`)
- Machine usage on libvirt: CPU time, memory, disk and network counters per device (`pi machine stats`)
- Control-plane health: role, term, commit index, follower lag, reconcile failures per module, commands queued per host, store size
- Prometheus endpoint `/metrics` with stable names and labels, behind a scrape token or a cloud admin's token
- Threshold alarms on any series, a machine's or a host's included
- Pod logs

Partial
- History: none; a server keeps the last 60 points per series in memory and Prometheus keeps the rest
- Host and machine series are on the leader only and start empty after a failover
- Machine usage: libvirt only; nothing for containers, cloud-hypervisor or bare metal
- Alarms: `for_seconds` is ignored; no CLI
- Events exist in-process with no API

Missing
- Alarm notifications and webhooks
- Usage metering and billing export
- Machine and host logs

## Control plane and operations

Done
- Three-voter consensus with majority-confirmed writes, compaction and snapshots. Lab-proven on three servers: an in-place upgrade, two joins by token, a leader killed and replaced in about two seconds, the old leader catching up
- Servers discover and join each other
- Agent enrollment with join tokens and client certificates
- Paging, label filters, revision checks on update
- CI: fmt, clippy, tests, sign-off

Partial
- Remove or replace a server: not possible; a dead one stays in the list
- Backup works; restore is copying a file by hand
- Upgrades: no schema version, no upgrade check
- TLS: the main listener is plain HTTP; HTTPS is a second port
- Command queue is in memory on the leader and lost on restart
- Health: `/healthz` only

Missing
- Rolling upgrade
- `/v1` prefix, OpenAPI document
- Rate limiting, idempotency keys
- Watch API
- Regions and multi-site

## Tooling

Done
- `pi` CLI: 29 nouns
- Web console: 29 pages
- Container images, tarball, installer, systemd units
- Release workflow (never run; there is no build yet)

Partial
- CLI has nothing for secrets, certificates, DNS, load balancers, buckets, shares, backups, alarms or metrics

Missing
- Terraform provider
- SDKs
- deb and rpm packages

## Order of work

1. Merge and prove PRs 109 and 110 on real Ceph and LVM. Fix what the three-server run found. Cut the first build.
2. Operations: server-to-server TLS, certificate renewal, remove a server, restore, schema version, durable command queue, evacuation.
3. What most sites expect: resize, cold migration, host maintenance, availability zones, server groups, OIDC, boot from volume, machine snapshot, volume encryption, load balancer on tenant networks, metadata on OVN, IPv6 router advertisements, metrics per machine, CLI for every resource, Terraform provider.
4. Long tail, on request: GPU and passthrough, UEFI and vTPM, bare metal deploy, trunks, QoS, VPN, BGP, DNSSEC, erasure coding, regions, billing.
