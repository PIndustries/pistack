# Status

What a private cloud needs, area by area, and where PiStack stands on each.

Read from the code on this branch at 71cf282, 2026-11-17. Each line is one of:

- **Done**: in the tree and reachable through the API.
- **Partial**: some of it works; the line says what does not.
- **Missing**: no code.

"No CLI" means it works over HTTP or `pi api` but `pi` has no command for it.

## Bugs found while reading

All fixed on main:

- #113: a project admin was a cloud admin; a disabled user's token and a deleted key's token kept working; a revoked agent certificate was accepted; the first admin password was logged; CORS was permissive.
- #114: the TCP load balancer used only the first member; on hostnat a port in several security groups got only the last group's rules.
- #115: clone and volume-from-snapshot gave a blank volume on LVM and RBD.
- #116: a machine could boot any project's image; a cloud-hypervisor guest wrote into the shared image and got no cloud-init. `pi machine create --volume` is refused with a message instead of ignored; attaching at start is still to do.
- #117: `/cluster`, including the full store snapshot, was open by default. It now needs a member or a join token.

Found by the three-server lab run and since fixed:

- #119: a failed network or load balancer reconcile was retried without pause, and a follower that fell behind a busy leader never caught up, after which no write was confirmed.
- A follower that fell behind a busy leader, or that once led and holds entries no majority confirmed, replaces its own store with a snapshot from the leader and starts again. It keeps its id and role, replaces at most once every ten minutes, and its unconfirmed entries are set aside, not merged.

## Compute

Done
- Create, delete, start, stop, reboot
- Sizes (flavors)
- Quota per project on machines, vCPUs, RAM and root disk
- cloud-init user data and SSH keys on libvirt
- Metadata service, EC2 layout, on overlay and hostnat
- Metadata service on OVN, through a namespace per network on each host: written and unit-tested, not yet run on hosts
- Live migration on libvirt (no CLI)
- Rebuild a stopped or running machine from an image: id, name, host, network and keys are kept; the root disk is replaced (`pi machine rebuild`)
- Resize a stopped machine to another size; the machine stays on its host, which must have room for what the new size adds (`pi machine resize`)
- Scheduler: host filters, label selector, spread weights
- Per-host resource accounting; a host that misses heartbeats is marked down
- Volume hot-plug on libvirt
- Host labels set by an admin; the `zone` label is the availability zone a machine can ask for
- Host label `pistack.io/disk`, written by the agent: `shared` when its machine disk directory is on an NFS or SMB mount. Anything else is `local`
- Disable and enable a host
- Runtimes: libvirt/KVM, cloud-hypervisor, youki containers, bare metal power, mock
- Pods, Deployments, StatefulSets, DaemonSets, Jobs, CronJobs, Services, Ingress, Autoscaler (loom)

Partial
- SSH keys: raw strings per machine, or a key pair kept as a resource of a project and given at create by name or id (`pi keypair`). A generated pair's private half is returned once and never stored
- VNC console: only when the VM is on the API server's own host; libvirt only; no serial console
- Console log: written on the host, no API to read it for a VM
- Affinity: soft hints by machine name; no server groups, no hard rule
- Host aggregates: labels and a host selector only; no aggregate resource
- Drain: live-migrates running virtual machines on libvirt hosts whose machine disks are shared; a machine that cannot be moved is listed with its reason and left running, and a local root disk or a volume on unshared storage is known before the host is asked
- Evacuate off a dead host: the host is powered off through its BMC, over IPMI or Redfish, and the off state read back before anything moves; without power control, or when the power-off fails, nothing moves until the operator confirms the host is dead and the report says so. Machines then start elsewhere from their image without the root disk; RBD volumes follow, local ones do not
- Compute quota: no GPU limit; a pod's machine is counted but a pod is not refused
- NIC hot-plug: the agent can do it, nothing on the server asks it to
- Boot from volume: the flag is stored, no runtime boots from one. Volumes named at create are refused with a message; `--near-volume` places the machine where a volume can attach and attaches nothing
- Root disk: one copy-on-write disk on libvirt; no extra ephemeral or swap disks
- GPU: in sizes and the scheduler; hosts always report zero and there is no passthrough
- Bare metal: power and PXE boot; does not write an image, no inspection or enrollment

Missing
- Cold migration
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
- Host image cache sweep and resumed downloads: the agent deletes a cached image nothing needs, and a broken transfer keeps its bytes and asks the server for the rest
- Quota per project on image count and stored bytes; the bytes are checked once an upload is in

Partial
- Minimum disk is checked against the size at machine create and rebuild; minimum RAM is stored and never checked
- Storing images in object storage: written, off by default (`[modules.mold] store = "silo"`, streamed); not run against a real cluster

Missing
- Image signing
- Image from volume

## Network

Backends: OVN, overlay (VXLAN and namespaces), hostnat (one host), mock.

Done
- Networks, subnets, allocation pools, ports, fixed IPs, MACs
- VPCs: a project's routed private space, the one router it owns, and the networks and subnets inside it (`pi vpc`, `pi vpc subnet`, and a console page)
- Routers, external gateway, SNAT (IPv4)
- Floating IPs on OVN, overlay and hostnat
- Automatic floating IPs from one marked external network, off by default, with an opt-out per machine and per project
- Security groups: stateful, ingress and egress, remote group, default group per project
- Port security toggle
- Provider networks, flat and VLAN
- Geneve or VXLAN on OVN; VXLAN on overlay
- DHCP on OVN and overlay
- Quota per project on networks, subnets, routers, security groups, ports, floating IPs and VPCs
- OVN gateway HA across chassis; distributed floating IPs

Partial
- IPv6: addresses and rules work; no router advertisements on OVN, SNAT is IPv4 only, hostnat ignores it
- MTU: accepted, not reconciled with tunnel overhead
- Static routes: OVN only; no CLI
- Instance DNS: only through a labelled zone; guests are not pointed at it
- Shared networks: cloud-wide or private, nothing between
- Allowed address pairs: honoured on OVN only; the other backends store the pair and change nothing on the host
- Overlay routers live on one host with no failover
- The CLI changes only a network's automatic-floating-IP setting (`pi network set`); no other network resource has an update command

Missing
- Trunk ports
- QoS and bandwidth limits
- VPN, firewall policies, BGP, port mirroring
- Per-project network sharing (RBAC)

Lab-proven on two hosts (OVN): networks, DHCP, traffic between hosts, a router with an external gateway, floating IPs in and out on both hosts, outbound NAT, security groups denying by default and allowing by rule, a flat provider network. Not yet run on hosts: VLAN provider networks, IPv6, gateway failover between hosts.

## Load balancers

Done
- Balancers, listeners, pools, members
- TCP, UDP, HTTP with host and path routes
- TLS termination from a stored certificate
- TCP and HTTP health checks
- Balancing by the pool's algorithm (round robin, least connections, source address) with member weights, on both data planes (`pi lb`)

Partial
- Runs inside the server process on the server's address; the VIP is never placed on a tenant network
- Session persistence is stored and ignored
- Connection limit works on HTTP only
- Floating IP field is unused

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
- NOTIFY both ways: a zone change is notified to its secondaries and to the addresses a primary names in `also_notify`, and a notified secondary pulls at once
- Automatic records for machines and floating IPs
- Zones and record sets from the command line (`pi dns zone`, `pi dns record`)

Partial
- Reverse DNS by hand only
- CLI has no update for a zone or a record set

Missing
- IXFR, TSIG, DNSSEC

## Block storage

Done
- Create, delete, attach, detach
- Snapshots, clone and volume from snapshot on local, LVM and RBD
- Volume from image
- Volume types
- Quota per project on volume count and gigabytes, snapshots, and backup count and gigabytes; extend is checked
- Backup to object storage and restore, local, LVM and RBD (`pi backup`)
- Backends: local file, LVM thin, Ceph RBD
- A local file or an LVM volume stays on the node that made it (`status.home_node`) and attaches only there; a machine created with `--near-volume` is placed on that node

Partial
- Extend works; an attached guest is not told the disk grew
- LVM and RBD have only been run against stand-in scripts, never real Ceph or LVM
- RBD credentials come from the host's `ceph.conf` and keyring, not from keep
- A local volume cannot be moved between nodes
- Transfer between projects: the API offers, accepts and cancels; `pi` has no command for it

Missing
- QoS
- Encryption at rest (flag stored, never read)
- Multi-attach
- Migration and retype
- Consistency groups
- Placement by free capacity
- iSCSI, NVMe, NFS backends

## Object storage

Done
- Buckets and objects
- Bucket create, delete and head over S3 as well as the JSON API
- S3 path-style: get (with range), put, head, delete, batch delete, copy, list v2, multipart
- SigV4, presigned URLs, streaming uploads
- Cleanup of abandoned multipart uploads
- Object bytes copied between control-plane servers, in full. A put is answered once another server holds the bytes (`[modules.silo] put_copies`, default 2); multipart staging is copied the same way, so an upload in flight survives a leader change

Partial
- S3 has no list v1, virtual-host style or tagging
- Versioning: set and read on the S3 surface, list every generation, read or delete one by versionId, and a plain delete writes a delete marker. Suspended is answered as versioning off, since the store keeps one flag. Through the S3 surface and the JSON API only: `pi bucket` sets versioning at create and cannot list or read a generation
- Quota: bucket count per project only; no limit on bytes
- A write that reaches a follower is carried to the leader unasked; a body over the carry limit is refused in S3 form and the client must send it to the leader

Missing
- ACLs and bucket policies
- Lifecycle and expiry
- Erasure coding
- Static websites

## Shared filesystems

Done
- NFS shares with access by client CIDR
- A share allowed in mode write is writable: managed exports carry `all_squash` to an unprivileged uid and the share directory is owned by it
- A ready share reports where to mount as `<address>:<path>` in its status, and `pi share show` prints it
- Shares are reconciled by the server that leads, and from the command line (`pi share`)

Partial
- CephFS share is created; access rules are stored and not applied
- NFS size is not enforced
- A share is exported by one server: the exports file and the directory stay local, so they do not follow a leader change

Missing
- Share snapshots, types, networks, resize

## Identity

Done
- Users, service accounts, projects, roles, role bindings
- Projects a token can work in, and a token traded for one scoped to another project (`pi project mine`, `pi project switch`)
- JWT tokens with expiry; API keys
- argon2id passwords
- A password being set must be at least twelve characters and not the name it is set for; a hash stored before the rule keeps working
- Quota framework: limits per project, set by an admin, read with usage by members (`pi quota`)
- Audit log: every change, login, secret read and refusal, with principal, API key, client address and the server of the cluster that carried the request, and outcome; read by a cloud admin with filters and paging, including by the carrying server (`pi audit list`); kept 90 days and 50,000 records by default
- Token signing keys with ids, rotated and retired by an admin (`pi signing-key`); the key servers recognise each other with is separate and does not rotate with them
- All of a user's tokens refused at once (`pi user revoke-tokens`)

Partial
- Roles: admin, member, reader only; custom roles grant nothing; the policy engine is built and unused
- Nested projects: parent recorded, nothing inherited
- Audit log: admin only, no per-project view; ordinary reads, object uploads and server joins are not recorded; no export or forwarding to another system
- Sign-in with an OpenID Connect identity token: a cloud admin registers a provider (issuer, client id, the claim naming a user, optionally a group claim mapped to roles and projects) and `POST /identity/oidc/login` takes a provider name and an ID token the client already holds. Keys come from the provider's discovery document over HTTPS and are cached ten minutes; RS256 and ES256 only; issuer, audience, expiry and not-before enforced. An outside identity is the provider and the token's subject, never matched to an account by name or email. This is the token check only: no browser sign-in and no CLI
- Sign-in lockout: ten failures for one name from one address hold that pair for fifteen minutes, and two hundred from anywhere hold the name for the window

Missing
- A single token cannot be revoked, only all of a user's at once
- Browser sign-in and CLI sign-in with OIDC: only the token check exists
- Signing-key rotation on a schedule: it is by hand
- Groups, domains
- SAML, LDAP, federation
- MFA

## Secrets

Done
- Encrypted secrets with versions
- Cloud CA, certificates, revocation list
- Root key in software or wrapped by Vault Transit
- Re-encryption under a new root key, offline, software backend (`pistack keep rotate-root-key`)
- Listener certificates renew while the server runs
- S3 access keys; join tokens with limits
- Secrets and certificates from the command line (`pi secret`, `pi cert`); a payload is never a command-line argument

Partial
- Agent certificates renew themselves, but only for an agent on HTTPS
- `rotate_after_days` on a secret and `root_key_rotate_after_days` only warn; nothing is rotated automatically
- No re-encryption under a new root key with the Vault backend
- The cloud CA cannot be rotated: issuing CA 5 years, root 10
- A host cannot be removed for good; the server of a removed control-plane server keeps its certificates, which are revoked by hand
- Vault tested against a mock and a dev server only

Missing
- PKCS#11

## Orchestration

Done
- Stacks, kits, workflows
- Apply, update, history, rollback
- Declared parameters substituted into the document at apply and update

Partial
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
- Alarms and metric series from the command line (`pi alarm`, `pi metric`)

Partial
- History: none; a server keeps the last 240 points per series in memory, none older than an hour, for at most 50,000 series, and Prometheus keeps the rest
- Host and machine series are on the leader only and start empty after a failover
- Machine usage: libvirt only; nothing for containers, cloud-hypervisor or bare metal
- Alarms: a `for_seconds` hold is measured on the stamps of the series' own samples, and can ask for at most the hour the window reaches
- Events exist in-process with no API

Missing
- Alarm notifications and webhooks
- Usage metering and billing export
- Machine and host logs

## Control plane and operations

Done
- Three-voter consensus with majority-confirmed writes, compaction and snapshots. Lab-proven on three servers: an in-place upgrade, two joins by token, a leader killed and replaced in about two seconds, the old leader catching up
- Servers discover and join each other
- A server taken out of the cluster by id, leader only, refused while the servers left could not form a majority without `--force` (`pi server remove`)
- A follower carries a client's write to the leader and returns the answer, forwarding everything the client sent but what belongs to the hop it leaves; the leader's audit record names the client and notes the carrier
- `pi --wait` follows its own write to the server that took it, and forgives a copy that is a few seconds behind
- Backup writes a checked copy of the store; restore puts a backup in place of a data directory's store and founds the cluster again with one server (`pistack store backup`, `pistack store restore`)
- Agent enrollment with join tokens and client certificates
- Paging, label filters, revision checks on update
- CI: fmt, clippy, tests, sign-off

Partial
- A server taken out of the cluster keeps its membership record in the store; its certificates are revoked by hand
- Upgrades: the store records a schema version and each server checks its own copy on open. Servers do not compare versions with each other, so a release that raises it needs every server stopped and upgraded together. No release has needed a migration yet; what changes between builds is written in the upgrade notes
- TLS: the main listener is plain HTTP; HTTPS is a second port
- Command queue is in memory on the leader. Machine start, stop and delete and volume attach and detach keep their id in the store and are issued again by the next leader, and the host runs each once. A live migration in flight at a leader change is not followed up
- Health: `/healthz` only

Missing
- Rolling upgrade
- `/v1` prefix, OpenAPI document
- Rate limiting, idempotency keys
- Watch API
- Regions and multi-site

## Tooling

Done
- `pi` CLI: 54 nouns
- Web console: 31 pages, with a page of its own for a host, pages for key pairs and VPCs, and panels for a project's limits and a user's roles
- The console is kept in small files, one per function, joined into the page at build time
- Rebuild, resize and key pairs have console pages, and the machine page follows the machine while Live is on
- Container images, tarball, installer, systemd units
- Release workflow: run; `26.11.13.1` published Linux x86_64 and aarch64 tarballs with `SHA256SUMS`. Container images are built by the workflow but none are published yet

Partial
- CLI has nothing for volume transfers

Missing
- Terraform provider
- SDKs
- deb and rpm packages

## Order of work

The plan is in [ROADMAP.md](ROADMAP.md).
