# Agents and hosts

A host is a machine that runs workloads. It runs `pistack agent`, which registers with the control plane, reports what the host has, and carries out commands.

```sh
pistack --data-dir /var/lib/pistack agent --join http://10.0.0.11:8443 --token pij_...
```

The agent only makes outbound connections. It long-polls the server for commands and posts results, all under `/rpc` on the API port. The server never connects to a host, so a host needs no open inbound port for the control plane.

## The state of agent authentication

Read this first, because the default configuration does not work end to end yet.

The design is: an admin creates a join token, the agent presents it once and receives a client certificate from the cloud CA, and from then on every `/rpc` call is authenticated by that certificate.

What happens today:

- By default the server requires a client certificate on `/rpc`.
- A client certificate can only be presented over HTTPS, so the agent would have to join through `--tls-listen`.
- The agent does not yet trust the cloud CA when it connects, so a join over HTTPS fails at the TLS handshake.
- A join over plain HTTP enrolls and receives the certificate, and is then refused with `401 missing client certificate` on the next call.

The only way to connect an agent today is to turn the certificate check off on the server:

```sh
PISTACK_RPC_INSECURE=1 pistack server --init --listen 10.0.0.11:8443 --insecure-http
```

With `PISTACK_RPC_INSECURE=1`, anything that can reach the API port can register as a host, with or without a join token. Add `PISTACK_REQUIRE_JOIN=1` to make every `/rpc` call carry a join token that is known, unexpired and not used up. The agent sends its token on each call, so give it a multi-use token in that mode. Either way, keep the API port on a private network until certificate authentication is finished.

## Join tokens

```sh
pi join-token create rack1
```

```text
join-token "rack1" created (jtok-32f70809)
token (shown once): pij_...
```

Only an admin can create one. The server stores a hash, so the token cannot be shown again.

| Option | Meaning |
| --- | --- |
| `--ttl-secs` | Lifetime. Default and maximum is 24 hours. |
| `--multi-use` | Allow more than one enrollment. |
| `--max-uses` | Number of enrollments for a multi-use token. Default and maximum 100. |

A token is single-use unless you say otherwise. `pi join-token list` shows uses and expiry. `pi join-token delete` revokes one. At most 100 unexpired, unspent tokens can exist at once. Expired and used-up tokens are removed after seven days.

Pass the token to the agent with `--token` or in `PISTACK_JOIN_TOKEN`.

## Enrollment and identity

On first start the agent sends its node name and the join token to `/rpc/agents/enroll`. The server spends one use of the token and issues a client certificate whose common name is the node name. The agent saves it in `<data_dir>/agent/` as `cert.pem`, `key.pem` and `ca.pem`. The certificate is valid for 365 days. The agent does not renew it.

On later starts the agent finds the certificate and needs no token.

The node name is the host's identity. It is chosen once, in this order, and written to `<data_dir>/node.name`:

1. `PISTACK_NODE_NAME`
2. `<data_dir>/node.name`
3. the common name of an existing agent certificate
4. the host name
5. a generated `node-<hex>`

Because the name is kept in the file, renaming the machine does not change the host record. Setting `PISTACK_NODE_NAME` to something new does: the control plane sees a new host. A certificate speaks only for the node named in it. If the saved certificate names a different node, the agent enrolls again when it has a token and refuses to start when it does not.

## What the host reports

At registration the agent reports the node name, vCPUs, memory, free disk, its runtimes and its underlay address. It then sends a heartbeat every 15 seconds with the machines it runs.

```sh
pi host list
pi host show agent1
```

```text
NAME    ID             PHASE  NODE    VCPUS  FREE-VCPUS  MEMORY-MIB  FREE-MIB  DRIVERS  LAST-SEEN
agent1  host-b35f1f72  ready  agent1  14     14          0           0         mock     2026-11-12T05:18:49.601670Z
```

That output is from an agent on a Mac, which is why it shows the mock driver and no memory. The `DRIVERS` column lists the host's capabilities and the runtimes the agent selected. Free capacity is the reported total minus the sizes of the machines placed there; it is not measured on the host.

A host that has not reported for 30 seconds goes to phase `error` with the message `no report for 30s`, and nothing new is placed on it. It returns to `ready` on its next report. Machines already on it are left alone.

The underlay address is the address other hosts use to reach this one for overlay networking. The agent takes it from `PISTACK_UNDERLAY_IP`, or else from the source address of the route to the server. It must be a private address (10/8, 172.16/12, 192.168/16 or 100.64/10). A machine that names a network is only placed on a host that reported one.

Hosts cannot be labelled, edited or removed through the API today. A machine's `--host-selector` therefore only matches the labels the server sets itself.

The agent reads memory from `/proc/meminfo`. On a system without it the host reports 0 MiB and nothing fits.

## Runtimes

The agent has one runtime per kind of machine. It decides at start by looking at the host.

| Kind | Runtime | Selected when |
| --- | --- | --- |
| `vm` | libvirt | `/dev/kvm` exists or `virsh` is on `PATH` |
| `container` | youki | `youki` is on `PATH` |
| `metal` | none | never, today |
| any | mock | see below |

### libvirt

Virtual machines run under libvirt and KVM on `qemu:///system`. The host needs `virsh`, `qemu-img` and `cloud-localds`. Disks go in `/var/lib/libvirt/images/pistack` (`PISTACK_LIBVIRT_DIR`). `PISTACK_VIRSH` names a different `virsh`.

### youki

Containers and pods run under [youki](https://github.com/youki-dev/youki), driven directly; there is no containerd. `PISTACK_YOUKI_BIN` names the binary if it is not on `PATH`.

How youki is run depends on who the agent is:

- As root, the agent runs youki directly.
- As another user with `PISTACK_YOUKI_SUDO=1`, it runs youki, `ip`, `ovs-vsctl` and the cleanup helper through `sudo -n`. Install `scripts/pistack-youki-cleanup` as `/usr/local/sbin/pistack-youki-cleanup` and allow those four commands in sudoers; the header of that script has the exact line.
- Otherwise it uses a rootless user namespace, if the kernel allows it.

Container state lives in `<data_dir>/youki`. `pi pod exec` and exec probes go through the same path.

### mock

The mock runtime keeps machines in memory and hands out addresses from `10.0.0.0/24`. Nothing runs.

The agent falls back to it by itself when the host has none of KVM, libvirt and youki, and logs a warning. On a host that has one of them, the kinds without a real runtime get the mock only when `PISTACK_ALLOW_MOCK=1` is set. The host shows `mock` in `DRIVERS` in both cases. If no runtime at all can be selected the agent exits.

### Present in the source but not reachable

The agent contains a Cloud Hypervisor runtime and a bare-metal runtime (IPMI and Redfish). The detection code never reports the capabilities that would select them, so `pistack agent` does not use them. `pi machine create --kind metal` is accepted, and the machine is then only placed on a mock host.

## Running the agent next to a server

A server and an agent on the same machine can share a data directory. They then share the node name. This is also the only arrangement in which locally stored images and volumes are visible to the agent; see [Storage](Storage).

## Environment

The variables the agent reads are listed under "Agent" in the [Configuration reference](Configuration-reference).
