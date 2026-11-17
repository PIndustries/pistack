# Known limits

What does not work yet, as of `main`. Each item was read in the source or seen on a running build. The list is long because the project is young, and it is kept here so nobody finds these out in production.

## Security

- **The API listener is plain HTTP.** `--tls-listen` adds HTTPS with a certificate from the cloud CA, but `pi` and agents do not trust that CA. `pi` needs `--insecure` to use it.
- **Agents cannot join with certificates.** Enrollment over HTTPS fails the TLS handshake, and over HTTP the agent cannot present the certificate it was issued. Agents only connect when the server runs with `PISTACK_RPC_INSECURE=1`, which lets anything that reaches the port register as a host.
- **Cluster traffic is encrypted only between servers that have a TLS listener.** By default (`PISTACK_CLUSTER_TLS=preferred`) a server with no `--tls-listen` is called over plain HTTP, and so is one whose TLS listener cannot be reached, so the member key and the snapshot can still cross the network in the clear. `required` closes that. See [Running a cluster](Running-a-cluster#tls-between-servers).
- **Servers do not show each other a certificate.** A server checks the certificate of the server it calls. The caller proves itself with the member key, which is one key for the whole cluster.
- **A join by discovery is plain HTTP.** Discovery announces the API address, so a server that joins that way needs `--insecure-join`.
- **TLS between servers was tried on one machine.** Three servers on loopback joined, replicated and elected with `required`. It has not been run across machines or through an upgrade from an older release.
- **`reader` is not read-only.** Only `gate` and `keep` enforce it. In the other modules a `reader` token can create and delete resources in its project. Fixed by [PIndustries/pistack#103](https://github.com/PIndustries/pistack/pull/103), not merged.
- **One token cannot be cancelled.** A token stops working when its principal is disabled or deleted, when the API key it came from is deleted, when its signing key is retired, or when it expires. `pi user revoke-tokens` refuses all of one user's tokens at once; there is no way to pick one.
- **The token signing key rotates only by hand.** `pi signing-key rotate` is an admin's command. Nothing runs it on a schedule.
- **Custom roles grant nothing.** Authorization knows only `admin`, `member` and `reader`. The policy engine in `crates/pistack-policy` is not wired in.
- **The root key is in the store by default.** With the `software` backend, a copy of `state.redb` is enough to read every secret. The Vault backend has not been run against a production Vault.
- **The cloud CA does not rotate.** The issuing CA lasts 5 years and the root 10. Every certificate in the cloud ends with the issuing CA, and there is no command or working manual procedure to replace it. See [Identity and access](Identity-and-access#how-long-the-cloud-ca-lasts).
- **Only an agent on HTTPS renews its certificate.** Renewal is proved with the certificate, and a certificate is only shown over HTTPS. Agents that connect over plain HTTP, which is the only way that works end to end today, keep the certificate they enrolled with.
- **A host cannot be removed for good.** There is no command for it. Revoke the host's certificates through `/secrets` to cut it off and stop it renewing.
- **A renewed certificate does not revoke the one before it.** The old one stays valid until its own date, about four months later.
- **Root key rotation is offline and for the software backend.** `pistack keep rotate-root-key` needs every server stopped and, in a cluster, the same key file on each. Under Vault only the wrapping key rotates.
- **Nothing checks that every server runs the same build before a signing key is rotated.** A server still on a build without key ids refuses the new tokens, and loses the cluster when the first key is retired.

## Control plane

- **A write is confirmed before it is answered, but the answer can still say no.** In a cluster with more than one voter the leader holds the answer until a majority of voters holds the entry, and a write no majority confirms within five seconds is answered `503`. The entry is in the leader's log by then and may still take effect, so a retry can apply it twice. There is no way to ask whether a write that was refused took. See [Running a cluster](Running-a-cluster#durability-of-a-write).
- **Only `pi` asks to be carried to the leader.** A write sent to a follower is refused with `409` and the leader's address. `pi` sends it again to that address, once, and can ask a follower to carry the request for it. Any other client has to do the same itself, and `pi` does not follow an address on a different scheme than the one it was given. A body over 8 MiB is not carried: the answer names the leader instead. An S3 write is the exception; it is carried without being asked, to the same 8 MiB.
- **At most three voters.** Further servers are learners. Two servers give one voter and one learner, with no failover.
- **Removing a server needs a leader.** When the majority of voters is gone there is no leader, and the dead servers cannot be removed.
- **Removal does not revoke secrets.** A removed server's store still holds the member key. The others refuse it by the id it states.
- **A live migration does not survive a change of leader.** If the leader restarts or is replaced before the source host has the command, the machine stays `updating` with the message `migrating <host>` and is not migrated. Machine start, stop and delete and volume attach and detach are carried over; see [Agents and hosts](Agents-and-hosts).
- **A request in flight fails when the leader changes.** `pi pod exec`, pod logs, and a volume attach or detach that was waiting for the host return an error. The attach is finished by the new leader. A detach is not: the volume stays attached until you detach it again, which is safe to repeat.
- **A server that cannot follow the log starts again.** A member that fell behind what the leader keeps, or that holds entries no majority confirmed, replaces its store with a snapshot and runs its binary again. The swap is not done inside the running process. See [Running a cluster](Running-a-cluster).
- **Unconfirmed writes on a deposed leader are set aside, not merged.** They stay in `state.redb.replaced-<time>` on that server. Nothing applies them again.
- **A store is replaced at most once in ten minutes.** A server the leader refuses again inside that time waits. The limit is not configurable.
- **Store replacement was tried on three local servers, not on separate machines.** The download is the same code a join uses.
- **Restore brings back one server.** `pistack store restore` puts a backup in place and that server becomes the only member. The other servers are emptied and joined again by hand. See [Running a cluster](Running-a-cluster).
- **Restore rolls back the store only.** Machines and volumes made after the backup are unknown to the restored store, and deleted ones are back in it. What the control plane does about the difference with the hosts has not been tested.
- **Servers do not compare schema versions.** Each store checks its own. A release that raises the version needs every server stopped and upgraded together.
- **Discovery is IPv4 and one /24.** It only runs when `--listen` is a private IPv4 address.
- **Only the store and object bytes are replicated.** Images, local volume files and share directories are files on one server's disk. Object bytes are copied to the other servers, and a volume on `local` or `lvm` stays on the one server that holds it.
- **The config file is mostly not applied.** Only `[modules.silo]` and `[modules.keep]` are read.
- **No watch API.** Clients poll.
- **Metrics have no history.** A server keeps the last 240 points of each series in memory, for at most an hour, and loses them when it restarts. History needs a Prometheus server scraping `/metrics`. See [Metrics and alarms](Metrics-and-alarms).
- **Host and machine metrics are on the leader only.** Agents report to the leader, so after a failover the new leader starts with none and fills up within a heartbeat or two.
- **Alarms tell nobody.** There are no notifications or webhooks. `for_seconds` is measured on the stamps of the samples of its series, so a hold can only be as long as the window reaches: an hour.
- **The audit log is for cloud admins only, and it is not complete.** A project member cannot read their own project's records. Ordinary reads, object uploads and a control-plane server joining are not recorded, and the client address is whatever connected to the server, so a proxy hides the real one. See [Identity and access](Identity-and-access#audit).
- **A pod is not refused by a quota.** The machine behind a pod counts towards its project's `machines`, `vcpus`, `memory_mib` and `disk_gib`, but the pod controller creates it without checking the limits.
- **Quota usage is shown from the last change.** The number `pi quota show` prints for a resource is written when something of that kind is created or deleted in the project. The check that refuses a request always counts the records.

## Hosts and runtimes

- **The agent is Linux only.** Elsewhere it reports no memory and nothing is placed on it.
- **Cloud Hypervisor and bare metal are not selectable.** The runtimes are in the source, and the agent never picks them. A machine of kind `metal` is only placed on a mock host.
- **Hosts cannot be removed.** A host that is gone for good stays in the list.
- **Drain only moves running virtual machines on libvirt hosts whose machine disks are shared.** Containers, pods, bare metal, stopped machines and machines whose disks are on the host itself are reported and left where they are.
- **Live migration does not copy disks.** It only works when the destination host already sees the machine's disk at the same path. A migration that the host refuses, or that never answers, leaves the machine `ready` where it was with `migrate did not happen:` and the host's words as its message. Another drain tries it again.
- **A machine whose disks are local to its host is not drained or migrated.** The agent says whether its machine disk directory is on an NFS or SMB mount, and the answer is `local` unless it says otherwise. A machine on such a host is listed by `pi host drain` with that reason, and a migrate call for it is refused. A machine with a volume on a backend the hosts do not share is listed the same way. Mount the disk directory where every host can see it and the host reports `shared` at its next hello. See [Agents and hosts](Agents-and-hosts#host-maintenance).
- **Evacuating a host loses root disks.** A root disk is a file on its host. An evacuated machine starts from its image, and `pi host evacuate` moves nothing without `--rebuild-from-image`.
- **Fencing needs power control.** An evacuation only proves a host is off through its BMC when a cloud admin has set `pi host power set` on it. Without power control, evacuation relies on `--confirm-dead`, and a host that is only cut off from the control plane runs its machines twice until its agent reports again.
- **A machine with a local or LVM volume is not evacuated.** The volume was on the dead host.
- **Pods pinned to a host by a volume are not drained or evacuated.**
- **Free capacity is bookkeeping.** It is the reported total minus what was placed, not a measurement. What a host really uses is reported as metrics and does not feed the scheduler.
- **Only libvirt machines report usage.** Containers, cloud-hypervisor guests and bare metal have no CPU, memory, disk or network series.
- **Container images are not pulled from registries.** An OCI image has to be uploaded as a tarball.

## Storage

- **No image distribution.** The agent is given a file path on the server. It works when the agent runs on the server machine, when the path is a shared mount, or when images are staged on each host by hand.
- **Local and LVM volumes work only while the server that holds them leads.** Such a volume records the node whose disk holds it and attaches there and nowhere else. After a failover the new leader has the record but not the bytes: a snapshot, extend, backup or restore of the volume fails and names the node that holds it, and a delete there drops the record and leaves the file behind. Nothing moves the bytes to the new leader. `pi machine create --near-volume` places a machine on the node that holds the volumes it names; an `rbd` volume has no home node and is not affected. See [Storage](Storage#which-server-holds-a-volume).
- **A volume cannot be resized below its size or below its source.** A volume made from a snapshot or another volume has to be at least as large as the source, and extend only grows.
- **Volume encryption does nothing.** The flag is stored.
- **The LVM and RBD backends have not been run against real storage.** The commands they issue are checked against scripts that log their arguments. Nothing here has been run against a Ceph cluster or a thin pool.
- **Object bytes are copied, not spread.** Another server has to hold a copy before a put is answered, and the rest catch up in the background. There is no erasure coding and no second storage system behind the bytes; the copies are files on control-plane servers.
- **The S3 API is partial.** No `ListBuckets`, no `ListObjects` v1, no ACLs, policies, lifecycle, tagging, CORS or server-side encryption, and no `versionId` on a copy source. Path-style addressing only. See [Storage](Storage#object-storage).
- **A versioned bucket has no way back to off.** `Suspended` is answered as a bucket that never had versioning: generations already kept are still listed and still read by `versionId`, and a plain delete stops writing markers. The store keeps one flag, not a third state.
- **A versioned bucket cannot be emptied in one call.** A plain delete leaves a marker, so the generations under it stay until each is deleted by its `versionId`, and until they are gone the bucket refuses `DeleteBucket`.
- **Share sizes are not enforced.** An NFS share is a plain directory with no quota on it. A CephFS subvolume is created at the size asked for and nothing in PiStack checks a write against it.
- **A share lives on the server that leads.** Its directory and its export line are local to that server and do not move on a failover; `status.export` names the address to mount. Followers neither export nor record status. See [Storage](Storage#shared-filesystems).
- **Every NFS client is squashed.** Every managed export line carries `all_squash`, with `anonuid` and `anongid` at one pair. A client is never seen as its own user or as root. The share directory is owned by that pair with mode `0770` and setgid, so it is not readable by any other account on the server itself.
- **No quota on object bytes.** A project can be limited in buckets and not in what it stores in them.
- **An image upload is checked against `images_bytes` after it is received.** The size is not known before, so an upload that does not fit is stored and then deleted.

## Networking

- **The default backend is `mock`.** A server started without `PISTACK_FABRIC_BACKEND` configures nothing on any host and reports success.
- **`hostnat` is single-host and IPv4 only.** Its helper needs five environment variables passed through `sudo`, and the repository ships no sudoers configuration for that.
- **`overlay` puts each project router on one host.** There is no redundancy for it. Instance metadata is redirected to a port only the server listens on, so it does not appear to work on hosts that run only an agent.
- **`ovn` has no load balancer of its own**, and PiStack does not install Open vSwitch or `ovn-controller` on hosts.
- **Metadata on OVN has not been run on hosts.** It is tested against stand-ins for `ip`, `ovs-vsctl` and the northbound database. [Networking](Networking) has the check to run on a lab.
- **Metadata on OVN needs a root agent and a leader.** An agent that is not root serves none. With no leader the guest gets `503`; the other backends read the server's local copy.
- **Metadata on OVN needs an on-link route in the guest.** A virtual machine gets it from its seed disk and a pod when it is plugged. A virtual machine created before the route was added to the seed keeps its old network config and has none: run `ip route replace 169.254.169.254 dev <interface>` in it, again after each restart, or create the machine again. A guest that sets its own addresses some other way has to add the route itself.
- **Metadata on OVN tells guests apart by network and source address.** Port security, on by default, is what stops a guest from sending with another guest's address. Turn it off on a port and that protection is gone for the guests that share its network and host.
- **Metadata on OVN is as open as the agent transport.** With client certificates, a host can read metadata only for the guests placed on it. With `PISTACK_RPC_INSECURE=1` anything that reaches the API port can ask for any guest's metadata, the secrets named on it included, as it can already register as a host.
- **Metadata requests are not rate limited.** One host serves at most 64 at a time per network; each one is a call to the leader.
- **A machine that is live-migrated has no metadata on the new host until that host's next heartbeat**, up to 15 seconds.
- **Automatic floating IPs come from one network for the whole cloud.** There is no pool per project, and the setting cannot be limited to some projects other than by labelling the ones that opt out.
- **An automatic floating IP does not follow a machine to a new address.** It stays on the port it was first given. Delete it and the next pass creates one for the new address.
- **Floating IPs given before the setting existed are not removed.** After an upgrade they stay until you delete them or their machine.
- **Load balancer traffic goes through the leader process.**
- **DNS defaults to port 5353**, which collides with mDNS on macOS and with avahi. Set `PISTACK_DNS_LISTEN`. The DNS server is authoritative only and does not forward.

## CLI

- **No commands for** certificate authorities. Use `pi api`.
- **`--project` does not re-scope a token.** A token's project is fixed at login.
- **`pi machine list --all` does not cross projects** for a token scoped to a project.
- **Dev mode creates resources with no project** when no token is sent. They are invisible after you log in.
- **In dev mode, anything attached to a network is not scheduled.** The mock host reports no underlay address.

## Packaging and documents

- **No distribution packages.** A release is a tarball for Linux x86_64 and aarch64 with the binaries, the systemd units and an install script, and two container images. There are no `.deb` or `.rpm` packages and nothing for macOS. See `docs/RELEASING.md`.
- **`docs/ROADMAP.md` in the repository is out of date** in both directions. Trust the code and this page over it.
