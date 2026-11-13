# Configuration reference

PiStack is configured with command-line flags and environment variables. A config file exists and is only partly wired in; see below.

This page lists every flag of `pistack` and every `PISTACK_*` variable found in the source as of `main`.

## pistack flags

### All commands

| Flag | Environment | Default | Meaning |
| --- | --- | --- | --- |
| `--data-dir <DIR>` | `PISTACK_DATA_DIR` | `/var/lib/pistack` | Where the state store, keys and module data live. |
| `--log <FILTER>` | `PISTACK_LOG` | `info` | Log filter, for example `info,gate=debug`. |

These go before the subcommand: `pistack --data-dir /srv/pistack server ...`.

### pistack server

| Flag | Environment | Default | Meaning |
| --- | --- | --- | --- |
| `--init` | | | Found a new cluster. Ignored when the data directory already has a store. |
| `--join <URL>` | | | Join the cluster whose leader is at this URL: the `https://` address of its `--tls-listen`, with `--join-ca-sha256`. Cannot be combined with `--init`. |
| `--join-ca-sha256 <HEX>` | `PISTACK_JOIN_CA_SHA256` | none | SHA-256 of the cloud's root CA certificate, as `pi join-token create` prints it. Needed to join an `https://` address. 64 hex digits; colons and a leading `sha256:` are accepted. |
| `--insecure-join` | `PISTACK_INSECURE_JOIN` | off | Let the first join go to an `http://` address. The join token and the store are sent in the clear. |
| `--listen <ADDR>` | `PISTACK_LISTEN` | `0.0.0.0:8443` | API listener. Plain HTTP. A private IPv4 address here enables discovery. |
| `--public-url <URL>` | `PISTACK_PUBLIC_URL` | derived from `--listen` | The URL clients, agents and other servers use to reach this server. |
| `--insecure-http` | | | Acknowledge that `--listen` is plain HTTP. Makes the default public URL `http://`. |
| `--tls-listen <ADDR>` | `PISTACK_TLS_LISTEN` | none | Additional HTTPS listener. Must be a specific address. Agents and the other servers call this one. |
| `--agent-url <URL>` | `PISTACK_AGENT_URL` | `https://<tls-listen>` | The URL agents are told to use for the HTTPS listener. Must be `https://`. Needs `--tls-listen`. |
| `--dev` | | | Dev mode: a mock local host, and requests without credentials act as admin. |

When `--public-url` is not given it is `https://<listen>`, or `http://<listen>` with `--insecure-http`, with `0.0.0.0` replaced by `127.0.0.1`. Other servers and agents are told this URL, so set it explicitly when listening on `0.0.0.0`.

A server with `--tls-listen` also records an agent URL in its member record. An agent that calls over HTTPS is told that URL for every server that has one, never the plain one. Set `--agent-url` when agents reach the HTTPS listener under another name or port than the one it binds.

### pistack agent

| Flag | Environment | Default | Meaning |
| --- | --- | --- | --- |
| `--join <URL>` | | required | The control plane's API URL. |
| `--token <TOKEN>` | `PISTACK_JOIN_TOKEN` | none | Join token for the first enrollment. |

### pistack store

| Command | Meaning |
| --- | --- |
| `pistack store backup --to <FILE>` | Copy the store to a new file and check the copy. Works while the server runs. |
| `pistack store restore --from <FILE>` | Check a backup and put it in the data directory as `state.redb`. The directory must be empty unless `--force` is given, which renames the store that is there. Server must be stopped. |
| `pistack store stats` | The schema version, rows and bytes per table, and the log range. Server must be stopped. |
| `pistack store dump` | Every resource as a line of JSON. Server must be stopped. |

### pistack keep

Root-key operations on the local store. Stop the server first.

| Command | Meaning |
| --- | --- |
| `pistack keep status` | Show where the root key is kept and whether the configured backend can open it. |
| `pistack keep migrate-to-vault` | Wrap the root key with Vault Transit and drop the plaintext copy. |
| `pistack keep migrate-to-software --allow-plaintext-key` | The reverse. |
| `pistack keep rewrap` | Re-encrypt the root key under the latest Transit key version. |

## Config file

The server looks for a TOML file in this order: the path in `PISTACK_CONFIG`, `./pistack.toml`, `/etc/pistack/pistack.toml`. No file is fine.

Only two sections are read today:

```toml
[modules.silo]
max_object_bytes = 5368709120     # largest single PUT, default 5 GiB
multipart_ttl_secs = 604800       # unfinished multipart uploads are removed after this, default 7 days
region = "pistack"                # region accepted in SigV4 signatures
replicate_interval_secs = 60      # each server pulls the object bytes it lacks this often; 0 turns it off
put_copies = 2                    # servers that hold the bytes before a put is answered; 1 does not wait

[modules.keep]
max_secrets_per_project = 1000
max_certificates_per_project = 500
max_access_keys_per_project = 100
max_live_join_tokens = 100        # unexpired, unspent join tokens at once
max_join_token_uses = 100         # most uses a multi-use join token may have
join_token_grace_secs = 604800    # expired or spent join tokens are deleted after this
```

The file format also defines `data_dir`, `node_name`, `[server]`, `[tls]` and `[log]`. The server does not apply them. Use the flags and variables on this page for those. Unknown top-level keys are an error, so a typo stops the server at start.

Any key can be set from the environment as `PISTACK__` followed by the path in upper case with `__` between segments:

```sh
PISTACK__MODULES__SILO__MAX_OBJECT_BYTES=1073741824
PISTACK__MODULES__KEEP__MAX_LIVE_JOIN_TOKENS=20
```

Note the two underscores after `PISTACK`. These are different from the single-underscore variables below.

## Environment variables: server

### General

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_DATA_DIR` | `/var/lib/pistack` | Same as `--data-dir`. |
| `PISTACK_LOG` | `info` | Same as `--log`. |
| `PISTACK_LISTEN` | `0.0.0.0:8443` | Same as `--listen`. |
| `PISTACK_PUBLIC_URL` | derived | Same as `--public-url`. |
| `PISTACK_TLS_LISTEN` | none | Same as `--tls-listen`. |
| `PISTACK_AGENT_URL` | derived | Same as `--agent-url`. |
| `PISTACK_CONFIG` | none | Path of the config file. |
| `PISTACK_NODE_NAME` | host name | Node name. Written to `<data_dir>/node.name` and kept from then on. |
| `PISTACK_CORS_ORIGINS` | none | Comma-separated origins whose pages may call the API from a browser, such as `https://portal.example`. With none, no cross-origin access is allowed; the console is same-origin and needs none. |
| `PISTACK_ADMIN_PASSWORD` | generated | Password of the bootstrap `admin` principal. Read only when the store has no principals. A generated one is written to `<data_dir>/admin-password`. |

### Audit log

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_AUDIT_MAX_AGE_DAYS` | `90` | Days an audit record is kept. `0` keeps records until the count limit drops them. |
| `PISTACK_AUDIT_MAX_RECORDS` | `50000` | Most audit records kept; the oldest are deleted first. Zero or a value that is not a number keeps the default. |

The leader enforces both every five minutes. See [Identity and access](Identity-and-access#audit).

### Metrics

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_METRICS_TOKEN` | none | A bearer token that may read `/metrics` and nothing else, for a Prometheus server. At least 16 characters; a shorter one is ignored with a warning. Without it `/metrics` takes a cloud admin's token only. |

See [Metrics and alarms](Metrics-and-alarms).

### Cluster and agent transport

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_RPC_INSECURE` | off | `1` stops `/rpc` from requiring a client certificate. Needed for agents today; see [Agents and hosts](Agents-and-hosts). |
| `PISTACK_REQUIRE_JOIN` | unset | `/cluster` always needs a member or a join token unless this is `0`, which opens it. `1` also makes every `/rpc` call carry a valid join token. |
| `PISTACK_JOIN_TOKEN` | none | The join token a new server sends on its first start. Also `pistack agent --token`. |
| `PISTACK_CLUSTER_TLS` | `preferred` | How servers call each other. `preferred` uses TLS to a server that has an HTTPS address and plain HTTP otherwise. `required` uses TLS only and refuses `/cluster/*` and `/silo/peer/*` on the plain listener; it needs `--tls-listen`. See [Running a cluster](Running-a-cluster#tls-between-servers). |
| `PISTACK_AUTO_RESYNC` | on | `0` stops a member from replacing its own store when the leader cannot bring it up to date from the log. It then logs what to do and waits. See [Running a cluster](Running-a-cluster#when-a-server-cannot-follow-the-log). |

`PISTACK_RPC_INSECURE` and `PISTACK_REQUIRE_JOIN=1` accept only the exact value `1`.

### Networking

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_FABRIC_BACKEND` | saved choice, else `mock` | `mock`, `hostnat`, `overlay` or `ovn`. See [Networking](Networking). |
| `PISTACK_OVN_NB` | `unix:/var/run/ovn/ovnnb_db.sock` | OVN northbound database, `unix:` or `tcp:`. |
| `PISTACK_OVN_GATEWAY_CHASSIS` | empty | Comma-separated gateway chassis names, in priority order. |
| `PISTACK_RELAY_BACKEND` | follows the fabric | `mock` or `forward`. Any other value stops the server. |
| `PISTACK_DNS_LISTEN` | `<private listen address>:5353`, else `127.0.0.1:5353` | DNS listener. `host:port`, or several separated by commas. |
| `PISTACK_METADATA_LISTEN` | `127.0.0.1:18880` | Instance metadata listener. `0.0.0.0` is refused. |
| `PISTACK_NODE_PORT_ADDRESS` | `127.0.0.1` | Address that service node ports bind. Must be loopback or private. |
| `PISTACK_CLOUDFLARED` | `cloudflared` on `PATH` | Binary for balancer listeners published through a Cloudflare tunnel. |
| `PISTACK_TAILSCALE` | `tailscale` on `PATH` | Binary for listeners published through Tailscale. |
| `PISTACK_WG` | `wg` on `PATH` | Binary for listeners published through WireGuard. |
| `PISTACK_IP` | `ip` on `PATH` | `ip` binary used with the WireGuard option. |

### Secrets

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_KEEP_BACKEND` | `software` | Where the root key lives: `software` (in the store) or `vault`. `pkcs11` is refused. |
| `PISTACK_KEEP_VAULT_ADDR` | none | Vault address. Required for `vault`. Must be `https`, or `http` to a loopback host. |
| `PISTACK_KEEP_VAULT_KEY` | none | Transit key name. Required for `vault`. |
| `PISTACK_KEEP_VAULT_TOKEN` | none | Vault token. Set this or the next one, not both. |
| `PISTACK_KEEP_VAULT_TOKEN_FILE` | none | File holding the Vault token. |
| `PISTACK_KEEP_VAULT_MOUNT` | `transit` | Transit mount path. |
| `PISTACK_KEEP_VAULT_NAMESPACE` | none | Vault namespace. |
| `PISTACK_KEEP_VAULT_CACERT` | none | CA certificate for Vault's TLS. |

Setting any `PISTACK_KEEP_VAULT_*` variable without `PISTACK_KEEP_BACKEND=vault` is an error. A store keyed by one backend does not start under the other. The Vault token is not renewed by PiStack.

### Storage

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_SLAB_DIR` | `<data_dir>/volumes` | Local volume files. |
| `PISTACK_MOLD_DIR` | `<data_dir>/images` | Image data. Also read by the youki runtime on a host. |
| `PISTACK_SHELF_DIR` | `<data_dir>/shares` | Share directories. |
| `PISTACK_LVM_BIN` | LVM tools on `PATH` | One binary to run in place of every LVM tool. |
| `PISTACK_RBD_BIN` | `rbd` | `rbd` binary for volume operations. |
| `PISTACK_QEMU_IMG_BIN` | `qemu-img` | `qemu-img` binary for filling a volume from an image. |
| `PISTACK_EXPORTS_FILE` | `/etc/exports.d/pistack.exports` | The NFS exports file the server manages. |
| `PISTACK_EXPORTFS` | `exportfs` on `PATH` | `exportfs` binary. |
| `PISTACK_SHELF_HELPER` | none | Privileged helper for a server that is not root. |
| `PISTACK_SHELF_RETRY_MS` | `5000` | Share reconcile interval. |
| `PISTACK_SHELF_BACKOFF_MS` | `5000` | Base retry delay after a failed export, capped at 300 seconds. |
| `PISTACK_CEPH_BIN` | `ceph` on `PATH` | `ceph` binary for CephFS shares. |
| `PISTACK_CEPH_FS` | none | CephFS filesystem name. Required for CephFS shares. |

## Environment variables: agent

### General

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_DATA_DIR` | `/var/lib/pistack` | Same as `--data-dir`. |
| `PISTACK_LOG` | `info` | Same as `--log`. |
| `PISTACK_JOIN_TOKEN` | none | Same as `--token`. |
| `PISTACK_NODE_NAME` | host name | Node name. A new value makes the control plane see a new host. |
| `PISTACK_UNDERLAY_IP` | source address of the route to the server | Address other hosts use to reach this one. Must be private. |
| `PISTACK_ALLOW_MOCK` | off | `1`, `true` or `yes` lets the mock runtime fill in for kinds with no real runtime. |

### Virtual machines

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_VIRSH` | `virsh` | `virsh` binary. |
| `PISTACK_LIBVIRT_DIR` | `/var/lib/libvirt/images/pistack` | Where virtual machine disks are kept. |
| `PISTACK_FIP_BIN` | `/usr/local/sbin/pistack-fip` | Helper used to turn on metadata redirection for the libvirt bridge. |
| `PISTACK_RBD_BIN` | `rbd` | `rbd` binary used to check RBD disks. |

### Containers

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_YOUKI_BIN` | `youki` on `PATH` | youki binary. |
| `PISTACK_YOUKI_STATE` | `<data_dir>/youki` | Container state directory. |
| `PISTACK_YOUKI_SUDO` | off | `1`, `true` or `yes`: an agent that is not root runs youki, `ip`, `ovs-vsctl` and the cleanup helper through `sudo -n`. |
| `PISTACK_YOUKI_CLEANUP` | `/usr/local/sbin/pistack-youki-cleanup` | Cleanup helper. |
| `PISTACK_YOUKI_NETNS` | none | Absolute path of a network namespace for containers to join. |
| `PISTACK_YOUKI_TIMEOUT_MS` | `30000` | Timeout for one youki command. |
| `PISTACK_IP_BIN` | `ip` | `ip` binary for pod networking. |
| `PISTACK_OVS_VSCTL_BIN` | `ovs-vsctl` | `ovs-vsctl` binary for plugging pods into OVN. |

### Volumes

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_VOLFS_SUDO` | off | `1`, `true` or `yes`: an agent that is not root runs the mount tools through `sudo -n`. |
| `PISTACK_VOLFS_DATA_DIR` | the data directory | Base directory for pod volume mounts (`slab/mounts` under it). |
| `PISTACK_LOSETUP`, `PISTACK_BLKID`, `PISTACK_MKFS`, `PISTACK_MOUNT`, `PISTACK_UMOUNT` | the system tools | Replace one of the mount tools. |
| `PISTACK_RBD` | `rbd` | `rbd` binary for mapping images for pods. |
| `PISTACK_RBD_SUDO` | off | `1`, `true` or `yes`: map and unmap through `sudo -n`. |

### Overlay networking

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_OVERLAY_BIN` | `/usr/local/sbin/pistack-overlay` | The overlay helper. |

## Environment variables: helper scripts

The helpers under `scripts/` run as root through `sudo` and read their own variables.

### pistack-fip (hostnat)

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_FIP_POOL` | required | CIDR that floating IPs come from. |
| `PISTACK_FIP_GUESTS` | required | Guest network CIDR. |
| `PISTACK_FIP_SOURCE` | required | Source CIDR allowed to reach floating IPs. |
| `PISTACK_FIP_IFACE` | required | Underlay interface that carries the floating IPs. |
| `PISTACK_FIP_HOST` | required | The host's own address on that interface. |
| `PISTACK_FIP_STATE` | `/var/lib/pistack/fip` | State directory. |

### pistack-overlay

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_OVERLAY_STATE` | `/var/lib/pistack/overlay` | State directory. |
| `PISTACK_OVERLAY_IP`, `PISTACK_OVERLAY_BRIDGE`, `PISTACK_OVERLAY_IPTABLES`, `PISTACK_OVERLAY_IP6TABLES`, `PISTACK_OVERLAY_SYSCTL`, `PISTACK_OVERLAY_DNSMASQ` | the system tools | Replace one of the tools the helper runs. |

## Runtimes that are not selectable yet

These are read by the Cloud Hypervisor and bare-metal runtimes, which `pistack agent` does not select today. They are listed so the names are not a surprise in the source.

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_CLOUD_HYPERVISOR_BIN` | `cloud-hypervisor` on `PATH` | Cloud Hypervisor binary. |
| `PISTACK_CH_REMOTE` | `ch-remote` | `ch-remote` binary. |
| `PISTACK_CLOUD_STATE` | `/var/lib/pistack/cloud` | State directory. |
| `PISTACK_CLOUD_KERNEL` | none | Kernel file to boot guests with. |
| `PISTACK_METAL_BMC` | IPMI | `redfish` selects Redfish. |
| `PISTACK_METAL_STATE` | `/var/lib/pistack/metal` | State directory. |
| `PISTACK_METAL_IMAGES` | `/var/lib/pistack/metal/images` | Image directory. |
| `PISTACK_METAL_CLEAN` | none | Wipe helper run when a machine is deleted. |
| `PISTACK_IPMI_DIR` | required | Directory of BMC password files, one per machine id. |
| `PISTACK_IPMITOOL` | `ipmitool` | `ipmitool` binary. |
| `PISTACK_CURL` | `curl` | `curl` binary for Redfish. |
| `PISTACK_REDFISH_SYSTEM` | `Systems/1` | Redfish system path. |

## Test hooks

These exist for the test suite. Do not set them on a real system.

`PISTACK_YOUKI_SUDO_BIN`, `PISTACK_VOLFS_SUDO_BIN`, `PISTACK_RBD_SUDO_BIN`, `PISTACK_YOUKI_MOUNTINFO`, `PISTACK_YOUKI_CLEANUP_ROOT`, `PISTACK_YOUKI_TEST_EUID`, `PISTACK_YOUKI_TEST_EGID`, `PISTACK_YOUKI_TEST_USERNS`, `PISTACK_VOLFS_TEST_EUID`, `PISTACK_RBD_TEST_EUID`, `PISTACK_RBD_DEV_ROOT`, `PISTACK_SHELF_EUID`, `PISTACK_SHELF_EXPORTS_DIR`, `PISTACK_DIRECT_EXPORTFS`, `PISTACK_EXPORT_FAIL`, `PISTACK_EXPORTFS_LOG`, `PISTACK_CEPH_LOG`, `PISTACK_CURL_LOG`, `PISTACK_IPMI_LOG`, `PISTACK_METAL_CLEAN_LOG`, `PISTACK_DOOR_LOG`, `PISTACK_HELPER_LOG`, `PISTACK_SUDO_LOG`, `PISTACK_TEST_VAULT_ADDR`, `PISTACK_TEST_VAULT_TOKEN`.

## pi

`pi` reads `PI_CONFIG`, `PI_PROFILE`, `PI_URL`, `PI_TOKEN` and `PI_PROJECT`. See the [CLI reference](CLI-reference).
