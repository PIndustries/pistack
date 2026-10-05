# Roadmap

Each phase ends at a gate an outside tool decides.

## Phase 0 — Foundation (now)

- [x] Cargo workspace, `pistack` binary, gateway, discovery document
- [x] `pistack-types`: resource model, errors, Module contract
- [x] `pistack-store`: redb-backed typed collections with name index, revisions, events
- [x] `pistack-policy`: rule language + evaluator
- [x] `pistack-config`: TOML + env + flags
- [x] `pistack-rpc`: host command queue (HTTP long-poll) and event bus
- [x] `gate`: projects, principals, roles, tokens, API keys, quotas; auth layer on the gateway; `--dev` mode
- [x] `keep`: cloud CA (root + issuing), encrypted secrets, certificates, join tokens, rustls server/client configs (gateway TLS wiring pending)
- `keep` root-key backends, chosen with `PISTACK_KEEP_BACKEND` (an unknown or parked value refuses to start; there is no fallback between backends):
  - [x] `software` (default): the AES-256-GCM root key is base64 in store meta `keep/root_key`.
  - [x] `vault`. The root key is wrapped and unwrapped with a Vault Transit key over HTTP (`PISTACK_KEEP_VAULT_ADDR`, `_TOKEN` or `_TOKEN_FILE`, `_KEY`, optional `_MOUNT`, `_NAMESPACE`, `_CACERT`); only the ciphertext is stored, in `keep/root_key_wrapped`. Operator commands, run on a stopped node with the server's environment: `pistack keep migrate-to-vault` (wrap, verify the round-trip, then write the wrapped key and delete the plaintext one in one store transaction), `pistack keep rewrap` (Transit rewrap to the latest key version after a rotation; run it before raising `min_decryption_version`, and startup explains the fix if you did not), `pistack keep migrate-to-software --allow-plaintext-key` (the reverse) and `pistack keep status`. A store holding both keys, or sealed data with no key, is refused at startup with the command that settles it. Tested against a mock Transit server, and against `vault server -dev` 2.1.1 with an ignored-by-default test. Not yet done: a lab run against a production Vault (TLS, namespace, least-privilege token). Limits: token auth only, the token is not renewed, and each control-plane node's store is migrated separately.
  - [ ] Parked: `pkcs11`. Refused with an error; not implemented.
- [x] `keep` limits: `secrets` and `certificates` per project use gate project quotas (a create at the limit is 403 `quota_exceeded`; a disabled project refuses), defaulting to `[modules.keep] max_secrets_per_project = 1000` and `max_certificates_per_project = 500` when the project names none. A certificate's key secret counts as part of the certificate. Usage is counted from stored rows and mirrored into the project's `status.usage`. Join tokens are cloud-scoped: at most `max_live_join_tokens = 100` unexpired, unspent tokens, and a multi-use token allows `max_uses` redemptions (default and cap `max_join_token_uses = 100`), enforced inside the store write so concurrent redeems cannot overshoot. Certificate description and labels have the same size limits as secrets. The leader deletes expired or used-up join tokens after `join_token_grace_secs` (default 7 days). Limits are exact cluster-wide in steady state because only the leader writes (followers return 409 for every mutation) and keep serialises count and create in that process. The store has no cross-key transaction, so during a leadership split, while an old leader has not yet seen the new term, each node accepting writes can admit up to the limit: at most `limit × nodes that accepted writes` on a copy that later merges both, and `max_uses × nodes` redemptions of one token.
- [x] `keep` access keys: an access key id (`PIAK…`, stored in the clear and indexed in store meta) and a 40-character secret, sealed as a keep-managed secret (`keep.pistack.io/role=access-key`; the whole `keep.pistack.io/` label prefix is locked) and bound to a project and a gate principal. `GET|POST /secrets/access-keys` and `GET|DELETE /secrets/access-keys/{id}`; the secret is in the create response only. `Client::lookup_access_key` returns the secret, project, principal and a project-scoped `Caller` (never `system`) for an in-process verifier, with one `invalid access key` error for unknown, revoked or otherwise unusable keys. Counted against a separate `access_keys` project quota (default `max_access_keys_per_project = 100`). CLI: `pi access-key` / `pi join-token` (bench). SigV4 verification in silo: see below. Not done: a last-used time (a lookup-path write would diverge on followers, which serve reads but cannot write).
- [x] `forge`: Host/Size/Machine, scheduler, reconciler, mock driver, RpcDriver + agent registrar
- [x] `anvil`: host facts, runtime trait with mock, real `pistack agent --join` loop
- [x] `loom`: Pod/ReplicaSet/Deployment/Service/ConfigMap/Job/CronJob/Ingress with six controllers on top of forge
- [x] `beacon`: zones, recordsets, authoritative DNS server (hickory), auto-records for machines
- [x] `relay`: balancers, listeners, pools, members, health checker, mock provider, OVN/Pingora mapping documented
- [x] `fabric`: Network/Subnet/Port/Router/SecurityGroup/FloatingIp, IPAM, mock backend, OVN mapping documented
- [x] `slab` + `mold`: Volume/Snapshot/Backup and Image with real local-file backends, qcow2 safety checks
- [x] `bench`: `pi` CLI covering 18 nouns, login/profiles, apply, completions. `/console` serves the web portal (login, machines, hosts, networks, volumes, images, buckets).
- [x] Gate reached 2026-10-02: `pistack server --init` + `pistack agent --join` in separate processes; `pi machine create --wait` reaches Ready on the agent's mock runtime; volumes attach; secrets and certs issue; a Deployment produces Running pods. 233 tests in CI.
- [x] Decisions for the next phase are recorded in [DECISIONS.md](DECISIONS.md): libvirt/KVM first, PiStack YAML stacks, the PiStack API only in v1, loom on the same records as every other module, youki directly under anvil
- [x] `silo`, `shelf`, `gauge`, `blueprint` are store-backed HTTP APIs. silo writes object bytes under the data dir (streaming PUT, default 5 GiB cap) and serves path-style S3 objects, ListObjectsV2, multipart upload, and ListMultipartUploads; unfinished multipart uploads are swept after `multipart_ttl_secs` (default 7 days). AWS SigV4 is not verified. gauge keeps an in-memory sample ring and Prometheus text. shelf shares stay Pending. blueprint apply accepts a document and does not create machines.
- [x] A request with no Caller is unauthorized (`no credentials`) on fabric, beacon, relay, blueprint, gauge, slab, mold, silo, and shelf. `--dev` still injects an admin only at the gateway. `/`, `/healthz`, `POST /identity/tokens`, and `/rpc` do not require a token.
- [x] Gateway TLS: `pistack server --tls-listen ADDR` (env `PISTACK_TLS_LISTEN`) serves the same app with rustls beside plain HTTP. Certs are `data_dir/tls/cert.pem`, `key.pem`, and `ca.pem`, reused when present and otherwise issued by keep. The address must be specific (not `0.0.0.0` or `::`). `--insecure-http` still only silences the plain-HTTP warning when TLS is not configured.
- [x] mTLS on `/rpc`: the TLS listener verifies client certificates against the cloud CA; verified peer identity reaches `/rpc`. Agents enroll with a spend-once join token (`POST /rpc/agents/enroll`) and present the issued certificate on hello/heartbeat/commands. Strict by default; set `PISTACK_RPC_INSECURE=1` only for local HTTP / tests. Optional `PISTACK_REQUIRE_JOIN=1` still authorizes `x-pistack-join` on every call.
- [x] Stable node identity: `<data_dir>/node.name` holds the one node name the agent, its client certificate, the forge host record, loom's `pistack.io/hostname`, and `ModuleContext::node_name` share. It survives restarts, host renames, and re-enrolls; `PISTACK_NODE_NAME` overrides and persists. `/rpc` binds the client certificate's common name to the node in each call.
- [x] Beacon DNS listen defaults: honor `PISTACK_DNS_LISTEN` (comma-separated multi-bind) when set; otherwise bind the server's private/cluster advertise address on `:5353`; fall back to `127.0.0.1:5353` when none is known. Defaults never use `0.0.0.0`.

## Phase 1 — Bootable cloud

- [x] libvirt/KVM driver in anvil (virsh, one NAT network `pistack` on 10.88.0.0/24, cloud-init seed). `pi volume attach` hotplugs a local slab file into that guest as a virtio disk.
- [x] Host NAT floating IPs when `PISTACK_FABRIC_BACKEND=hostnat`. `pi fip associate --machine` uses the guest address.
- [x] Overlay datapath when `PISTACK_FABRIC_BACKEND=overlay`: one VXLAN segment per network, one router namespace per router, static forwarding to peer underlays, DHCP and subnet routes from that router, a provider bridge for a physical link, floating-IP translation on the host that owns the port, and placement only onto a host that reported an underlay address. Host NAT stays in place for networks already realised that way.
- [ ] LVM and Ceph RBD in slab; qemu-img in mold. Attach uses the local file backend.
- TLS everywhere via keep
- Gate: a VM boots on three nodes and gets an IP, a volume and a floating IP from the CLI.

## Phase 2 — Containers and platform

- [x] youki runtime: an OCI bundle and `youki create` / `youki start` when the binary is on the host. A missing binary is unavailable.
- [x] Stack apply writes `network`, `subnet`, and `machine` rows from the stack document. `pi apply` already writes one resource at a time.
- [x] L4 forwarder when `PISTACK_RELAY_BACKEND=forward`: a loopback or private listener copies bytes to a pool member.
- [x] HTTP reverse proxy for the same setting. It answers `GET` from an up member. TLS termination stays unavailable until a certificate is configured. UDP is refused.
- [x] A listener door runs cloudflared or tailscale when that binary is present and stays unavailable when it is missing. WireGuard and an edge host start no process.
- [x] A failed live migration sets the machine to `Error` and leaves it on the source host.
- [x] Host heartbeats write memory and vcpu capacity into one process-wide gauge ring.
- [x] An NFS share is `Ready` only after `exportfs` accepts the client CIDR. CephFS is `Ready` only after `ceph fs subvolume create`. A missing tool stays `Pending`.
- [x] A metal machine that names this host is the host and is not powered off. A different machine with a BMC address is powered with `ipmitool` when that binary and a password file are present.
- [ ] A stack with a VM database and a pod web tier behind one relay and one DNS name, on real hosts.
- beacon (hickory) and silo are already in the tree.

## Phase 3 — Everything else

- shelf, blueprint kits, Cloud Hypervisor driver, bare-metal provisioning, web console. Control-plane discovery, adopt, and leader replication are part of `pistack server`. A compatibility door waits until a migration needs it.
