# Known limits

What does not work yet, as of version 0.1.0 on `main`. Each item was read in the source or seen on a running build. The list is long because the project is young, and it is kept here so nobody finds these out in production.

## Security

- **The API listener is plain HTTP.** `--tls-listen` adds HTTPS with a certificate from the cloud CA, but `pi` and agents do not trust that CA. `pi` needs `--insecure` to use it.
- **Agents cannot join with certificates.** Enrollment over HTTPS fails the TLS handshake, and over HTTP the agent cannot present the certificate it was issued. Agents only connect when the server runs with `PISTACK_RPC_INSECURE=1`, which lets anything that reaches the port register as a host.
- **Cluster routes are open by default.** `/cluster` (membership, snapshot download, join) needs no credentials unless `PISTACK_REQUIRE_JOIN=1` is set. A snapshot is the whole store.
- **`reader` is not read-only.** Only `gate` and `keep` enforce it. In the other modules a `reader` token can create and delete resources in its project. Fixed by [PIndustries/pistack#103](https://github.com/PIndustries/pistack/pull/103), not merged.
- **A project admin is a cloud admin.** Any token carrying the `admin` role passes every admin check, including for bindings made in a single project.
- **Tokens cannot be revoked.** Disabling or deleting a principal does not invalidate tokens already issued. The maximum lifetime is 7 days.
- **Custom roles grant nothing.** Authorization knows only `admin`, `member` and `reader`. The policy engine in `crates/pistack-policy` is not wired in.
- **The root key is in the store by default.** With the `software` backend, a copy of `state.redb` is enough to read every secret. The Vault backend has not been run against a production Vault.
- **Agent certificates are not renewed.** They are valid for 365 days.

## Control plane

- **A write is durable on the leader only.** The leader answers before any follower has the entry. Losing the leader's disk at the wrong moment loses the write. Also addressed by #103.
- **`pi` does not follow the leader.** A write sent to a follower fails with `409` and the leader's address. You have to point `pi` at the leader.
- **At most three voters.** Further servers are learners. Two servers give one voter and one learner, with no failover.
- **No way to remove a member.** A server that is gone for good stays in the member list.
- **Rejoining a server that fell behind is manual.** See [Running a cluster](Running-a-cluster).
- **No restore command.** Restoring a backup means putting the file in place by hand.
- **Discovery is IPv4 and one /24.** It only runs when `--listen` is a private IPv4 address.
- **Only the store is replicated.** Images, local volumes, objects and shares are files on one server's disk.
- **The config file is mostly not applied.** Only `[modules.silo]` and `[modules.keep]` are read.
- **No watch API.** Clients poll.
- **The audit log cannot be read through the API.**

## Hosts and runtimes

- **The agent is Linux only.** Elsewhere it reports no memory and nothing is placed on it.
- **Cloud Hypervisor and bare metal are not selectable.** The runtimes are in the source, and the agent never picks them. A machine of kind `metal` is only placed on a mock host.
- **Hosts cannot be labelled, drained or removed.** The host API is read-only.
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
