# Known limits

What does not work yet, as of `main`. Each item was read in the source or seen on a running build. The list is long because the project is young, and it is kept here so nobody finds these out in production.

## Security

- **The API listener is plain HTTP.** `--tls-listen` adds HTTPS with a certificate from the cloud CA, but `pi` and agents do not trust that CA. `pi` needs `--insecure` to use it.
- **Agents cannot join with certificates.** Enrollment over HTTPS fails the TLS handshake, and over HTTP the agent cannot present the certificate it was issued. Agents only connect when the server runs with `PISTACK_RPC_INSECURE=1`, which lets anything that reaches the port register as a host.
- **Cluster traffic is not encrypted.** `/cluster` answers only a member or a server with a join token, but servers talk to each other over plain HTTP, so the member key and the snapshot cross the network in the clear. Keep that network private.
- **`reader` is not read-only.** Only `gate` and `keep` enforce it. In the other modules a `reader` token can create and delete resources in its project. Fixed by [PIndustries/pistack#103](https://github.com/PIndustries/pistack/pull/103), not merged.
- **The token signing key does not rotate.** A token stops working when its principal is disabled or deleted, when the API key it came from is deleted, or when it expires. There is no way to cancel one token on its own.
- **Custom roles grant nothing.** Authorization knows only `admin`, `member` and `reader`. The policy engine in `crates/pistack-policy` is not wired in.
- **The root key is in the store by default.** With the `software` backend, a copy of `state.redb` is enough to read every secret. The Vault backend has not been run against a production Vault.
- **Agent certificates are not renewed.** They are valid for 365 days.

## Control plane

- **A write is durable on the leader only.** The leader answers before any follower has the entry. Losing the leader's disk at the wrong moment loses the write. Also addressed by #103.
- **`pi` does not follow the leader.** A write sent to a follower fails with `409` and the leader's address. You have to point `pi` at the leader.
- **At most three voters.** Further servers are learners. Two servers give one voter and one learner, with no failover.
- **Removing a server needs a leader.** When the majority of voters is gone there is no leader, and the dead servers cannot be removed.
- **Removal does not revoke secrets.** A removed server's store still holds the member key. The others refuse it by the id it states.
- **Rejoining a server that fell behind is manual.** See [Running a cluster](Running-a-cluster).
- **Restore brings back one server.** `pistack store restore` puts a backup in place and that server becomes the only member. The other servers are emptied and joined again by hand. See [Running a cluster](Running-a-cluster).
- **Restore rolls back the store only.** Machines and volumes made after the backup are unknown to the restored store, and deleted ones are back in it. What the control plane does about the difference with the hosts has not been tested.
- **Servers do not compare schema versions.** Each store checks its own. A release that raises the version needs every server stopped and upgraded together.
- **Discovery is IPv4 and one /24.** It only runs when `--listen` is a private IPv4 address.
- **Only the store is replicated.** Images, local volumes, objects and shares are files on one server's disk.
- **The config file is mostly not applied.** Only `[modules.silo]` and `[modules.keep]` are read.
- **No watch API.** Clients poll.
- **The audit log cannot be read through the API.**

## Hosts and runtimes

- **The agent is Linux only.** Elsewhere it reports no memory and nothing is placed on it.
- **Cloud Hypervisor and bare metal are not selectable.** The runtimes are in the source, and the agent never picks them. A machine of kind `metal` is only placed on a mock host.
- **Hosts cannot be removed.** A host that is gone for good stays in the list.
- **Drain only moves running virtual machines on libvirt hosts.** Containers, pods, bare metal and stopped machines are reported and left where they are.
- **Live migration does not copy disks.** It only works when the destination host already sees the machine's disk at the same path. A failed migration leaves the machine in phase `error` on the old host, still running.
- **Evacuating a host loses root disks.** A root disk is a file on its host. An evacuated machine starts from its image, and `pi host evacuate` moves nothing without `--rebuild-from-image`.
- **Evacuation does not fence.** PiStack cannot power off a host. If an evacuated host is still running its machines, they run twice until its agent reports again, and both copies write to any shared volume.
- **A machine with a local or LVM volume is not evacuated.** The volume was on the dead host.
- **Pods pinned to a host by a volume are not drained or evacuated.**
- **The web console has no host controls.** Label, disable, drain and evacuate are in `pi` and the API only.
- **Free capacity is bookkeeping.** It is the reported total minus what was placed, not a measurement.
- **Container images are not pulled from registries.** An OCI image has to be uploaded as a tarball.

## Storage

- **No image distribution.** The agent is given a file path on the server. It works when the agent runs on the server machine, when the path is a shared mount, or when images are staged on each host by hand.
- **Local and LVM volumes are tied to the server machine** for the same reason. RBD volumes are not.
- **Snapshot restore and clone are blank on LVM and RBD.** A volume created from a snapshot or from another volume only contains the data on the `local` backend.
- **Backups of RBD volumes cannot be restored.**
- **Volume encryption does nothing.** The flag is stored.
- **Volumes cannot shrink.**
- **Object storage keeps one copy on one server.** No replication, no erasure coding.
- **The S3 API is partial.** No bucket operations through S3, no `ListObjects` v1, no range reads, no batch delete, no ACLs, policies, lifecycle, tagging, CORS or server-side encryption. Path-style addressing only.
- **NFS share sizes are not enforced.**

## Networking

- **The default backend is `mock`.** A server started without `PISTACK_FABRIC_BACKEND` configures nothing on any host and reports success.
- **`hostnat` is single-host and IPv4 only.** Its helper needs five environment variables passed through `sudo`, and the repository ships no sudoers configuration for that.
- **`overlay` puts each project router on one host.** There is no redundancy for it. Instance metadata is redirected to a port only the server listens on, so it does not appear to work on hosts that run only an agent.
- **`ovn` is half built.** The northbound database is written. PiStack does not set up `ovn-controller` on hosts, virtual machines under libvirt are not plugged into OVN, and there is no metadata service or OVN load balancer.
- **Load balancer traffic goes through the leader process.**
- **DNS defaults to port 5353**, which collides with mDNS on macOS and with avahi. Set `PISTACK_DNS_LISTEN`. The DNS server is authoritative only and does not forward.

## CLI

- **No commands for** DNS, load balancers, secrets and certificates, buckets, shares, volume backups, or metrics. Use `pi api`.
- **`--project` does not re-scope a token.** A token's project is fixed at login.
- **`pi machine list --all` does not cross projects** for a token scoped to a project.
- **Dev mode creates resources with no project** when no token is sent. They are invisible after you log in.
- **In dev mode, anything attached to a network is not scheduled.** The mock host reports no underlay address.

## Packaging and documents

- **No releases, packages, container images or service units.** Build from source.
- **`docs/ROADMAP.md` in the repository is out of date** in both directions. Trust the code and this page over it.
