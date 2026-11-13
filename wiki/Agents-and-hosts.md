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

At registration the agent reports the node name, vCPUs, memory, free disk, its runtimes and its underlay address. It then sends a heartbeat every 15 seconds with the machines it runs and what the host and those machines are using. See [Metrics and alarms](Metrics-and-alarms).

```sh
pi host list
pi host show agent1
```

```text
NAME    ID             PHASE  SCHEDULING  NODE    VCPUS  FREE-VCPUS  MEMORY-MIB  FREE-MIB  DRIVERS  ZONE  LAST-SEEN
agent1  host-b35f1f72  ready  enabled     agent1  14     14          0           0         mock           2026-11-12T05:18:49.601670Z
```

That output is from an agent on a Mac, which is why it shows the mock driver and no memory. The `DRIVERS` column lists the host's capabilities and the runtimes the agent selected. Free capacity is the reported total minus the sizes of the machines placed there; it is not measured on the host.

A host that has not reported for 30 seconds goes to phase `error` with the message `no report for 30s`, and nothing new is placed on it. It returns to `ready` on its next report. Machines already on it are left alone until an admin evacuates the host; see [Evacuate](#evacuate-a-dead-host).

The underlay address is the address other hosts use to reach this one for overlay networking. The agent takes it from `PISTACK_UNDERLAY_IP`, or else from the source address of the route to the server. It must be a private address (10/8, 172.16/12, 192.168/16 or 100.64/10). A machine that names a network is only placed on a host that reported one.

The agent reads memory from `/proc/meminfo`. On a system without it the host reports 0 MiB and nothing fits.

## Commands and a change of leader

The leader tells a host what to do by putting a command on that host's queue. The agent fetches commands with its long poll, runs them in order, and posts the results. A command that gets no answer within 120 seconds fails, and a host can have at most 1024 commands waiting to be fetched. Further ones are refused until it polls.

The queue is memory on the leader, so it is empty after the leader restarts or another server takes over. Nothing is lost by that for a machine start, stop or delete, or for a volume attach or detach:

- The new leader works out what is outstanding from the resources themselves. A machine that is `creating` is started, one that is `deleting` is deleted, one whose power differs from what was asked is stopped or started, and a volume that is `Attaching` is attached.
- Each of these commands has an id that is written to the store when it is issued and removed when it is answered. The command sent again by the new leader has the id the first one had.
- The agent keeps the result of each such command for an hour, at most 256 of them, in `commands-done.json` in its data directory, so it still has them after it restarts. When an id it has completed arrives a second time, it does not run the command again. It answers with the result it kept.
- A result the agent could not deliver, because the server was gone, is sent to the server it reaches next.

So a start that the old leader sent and the host ran is reported to the new leader and is not run twice.

The store holds one small record for each command that is waiting for its answer: the id, the kind and the time, under `rpc/commands/<node>/` in the meta table. A record lives no longer than its command, 120 seconds. A host that is down for a long time therefore leaves nothing behind, and what was asked of it is asked again when the reconcilers next find it outstanding.

Evacuating a host drops every command that was waiting for it.

Not covered: a live migration, and `pi pod exec` and pod logs, which fail and have to be asked for again. See [Known limits](Known-limits). An overlay plan is not carried over either and does not need to be: every new leader sends each host its current plan.

## Host maintenance

Everything in this section needs a cloud admin. An `admin` of one project is refused.

### Labels and zones

```sh
pi host label agent1 zone=west rack=r2
pi host label agent1 --remove rack
pi host label agent1 --replace zone=east
```

A label is a `key=value` pair on a host. Keys and values are 1 to 63 characters of letters, digits, `.`, `-`, `_` and `/`. `--remove` takes one label off. `--replace` drops every label not named on the command line.

Labels that start with `pistack.io/` belong to PiStack. You cannot set them, and `--replace` keeps them.

A machine's `--host-selector key=value` only matches a host that has that label.

The label `zone` is the availability zone. `pi machine create --zone west` places the machine only on a host labelled `zone=west`. A host with no `zone` label is in no zone. A machine that asks for a zone no host has stays `pending` and says so.

The API call is `PUT /compute/hosts/{id}/labels` with `{"labels": {...}}`. It replaces the whole set.

### Disable and enable

```sh
pi host disable agent1 --reason "replacing a fan"
pi host enable agent1
```

A disabled host gets no new machines. The machines on it keep running, and you can still stop, start, reboot and delete them. The host stays `ready`.

`pi host list` shows `disabled` and the reason in the `SCHEDULING` column. `pi host show` shows `spec.disabled` and `spec.disabled_reason`.

Disabling is the operator's setting. An agent restart does not clear it.

### Drain

```sh
pi host drain agent1 --reason "kernel update"
```

```text
host "agent1" disabled
machine "web" migrating to agent2
machine "cache" not moved: is a container; only a virtual machine can be live-migrated
2 still on host "agent1": 1 migrating, 1 not moved. Run the command again to check.
```

Drain disables the host and then live-migrates its machines to other hosts. The scheduler picks each destination, so selectors, zones and sizes still apply.

Drain never stops a machine. A machine it cannot move stays where it is, running, and is listed with the reason. These are not moved:

- containers, pods and bare metal
- a machine that is powered off or not `ready`
- any machine when the host has no libvirt runtime
- a machine no other host can take

A migration takes time. The machine leaves the list when its new host reports it. Run the command again to see what is still on the host. Running it again is always safe: it does not start a second migration for a machine that is already moving.

When a migration fails, the machine goes to phase `error` with the message from libvirt. It is still running on the old host.

Live migration does not copy disks. The destination host must already see the machine's disk at the same path.

When the work is done, `pi host enable agent1` puts the host back.

### Evacuate a dead host

```sh
pi host evacuate agent1 --rebuild-from-image
```

```text
WARNING: ROOT DISKS ARE LOST. The machines below start on other hosts from their images. What was on their root disks stayed on host "agent1" and is deleted when that host reports again.
machine "web" evacuated: root disk lost, starts from image ubuntu-24.04; volumes plugged back in when it runs: blocks
machine "db" skipped: volume scratch is on local storage on this host and cannot follow the machine
1 evacuated, 1 skipped
```

Evacuate is for a host that is gone. It gives the host's machines back to the scheduler, which starts them on other hosts. Drain is for a host that works.

**An evacuated machine loses its root disk.** The root disk is a file on the host. Nothing can copy it off a dead host. The machine starts again from its image. It keeps its name, its id and its ports on project networks, with their addresses. Anything written to the root disk is gone.

Because of that, no machine is evacuated unless you pass `--rebuild-from-image`. Without the flag the command only lists what it would have to rebuild.

What happens to volumes:

- A volume on shared storage (an `rbd` volume type) follows the machine. It is plugged into the machine again once the machine runs on its new host.
- A volume on `local` or `lvm` storage lived on the dead host. A machine with one is skipped and named.

These are also skipped: bare metal machines, pods that are pinned to the host by their volumes, and machines that are being deleted or live-migrated.

Evacuate only acts on:

- a host in phase `error`, which means it stopped reporting, or
- a disabled host, when you add `--confirm-dead`.

It refuses a host that is up.

**PiStack cannot power off a host.** Evacuate trusts you that the machines there are not running. If the host is only cut off from the control plane, its machines keep running there while new copies start elsewhere. Two copies then use the same addresses, and both write to any shared volume. Make sure the host is off before you evacuate it.

When the host comes back, its agent still has the old copies. Each heartbeat reports them, and the control plane tells the agent to delete every machine that was evacuated off it. That removes the old root disks too. If you need data from an old root disk, copy it off the host before you start the agent there.

A machine that was evacuated has the condition `Evacuated` and the annotation `pistack.io/evacuated-from`.

Running the command again is safe. It only sees the machines that were skipped the first time.

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
