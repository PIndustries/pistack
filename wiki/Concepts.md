# Concepts

## One binary, two roles

`pistack server` is a control-plane node. It opens the state store, serves the API on one port, and runs the reconcilers that make the world match what you asked for.

`pistack agent` runs on each host that carries workloads. It calls the server, picks up commands (start this machine, attach this volume) and reports back. The server never connects to a host.

`pi` is the client. Everything it does goes through the HTTP API, and `pi api` lets you call any path directly.

## Resources

Everything you create is a resource with the same shape:

```yaml
id: mach-01a1ce2d-6106-712b-a3e7-4fb369655f47
name: web
revision: 4
created_at: 2026-11-12T05:14:02.886135Z
updated_at: 2026-11-12T05:14:02.919356Z
spec:
  kind: vm
  size: m.2x4
  image: ubuntu-24.04
  power: on
status:
  phase: ready
  host: host-01a1ce2c-c8e9-72cf-835c-0d60365709b6
  addresses:
  - 10.0.0.1
```

- `spec` is what you asked for. You write it.
- `status` is what is true. Only reconcilers write it.
- The `id` is the kind prefix and a UUIDv7. `pi` shows a short form (`mach-69655f47`).
- A resource can also carry a `description`, `labels` and `annotations`.

A create or update returns as soon as the spec is stored. The work happens afterwards, and `status.phase` and `status.conditions` tell you how far it got. `--wait` on `pi` commands polls until the resource settles.

Names are unique per kind within a project. Anywhere the API or `pi` takes an id, it also takes the name.

### Revisions

`revision` starts at 1 and goes up by one on every change. An update may send `If-Match: <revision>`. If the resource has changed since, the server answers `409` with the code `conflict`, and you read it again and retry. Without `If-Match` the update is applied unconditionally.

### The HTTP API

Each module is mounted under a prefix on the one port. Paths are plain: `/compute/machines`, `/compute/machines/web`, `/network/networks`. There is no version segment and no project in the path. The project comes from the token.

- `GET /` lists the mounted modules.
- `GET /healthz` answers `ok`.
- Lists take `limit` (default 100, at most 1000), `after` (the last id of the previous page), `name`, and `labels=k=v,k2=v2`. The answer is `{"items": [...], "next": ..., "total": ...}`.
- Errors look like `{"error": {"code": "...", "message": "...", "details": {...}}}`.
- There is no watch endpoint. Clients poll.

## Projects

A project is the unit of ownership. Most resources belong to exactly one. A project can have a parent, quotas and an enabled flag.

A new cloud has one project, `admin`. It cannot be deleted.

A token is scoped to one project, chosen at login. That scope decides which resources a request sees and where new ones go. See [Identity and access](Identity-and-access).

Some resources are cloud-wide and have no project: hosts, sizes, roles, principals and join tokens.

### Quotas

A project's quotas are a map of resource name to limit. A name with no entry is unlimited. A request that would take the project over a limit is refused with `403` and the code `quota_exceeded`. The message names the resource, what is used, what was asked for and the limit, and `details` carries the same four values:

```json
{"error": {"code": "quota_exceeded",
           "message": "quota exceeded for vcpus in project 'research': 14 used + 4 requested > 16",
           "details": {"resource": "vcpus", "limit": 16, "used": 14, "requested": 4}}}
```

| Resource | What is counted | Checked when |
| --- | --- | --- |
| `machines` | Machines | A machine is created |
| `vcpus`, `memory_mib`, `disk_gib` | The size of every machine, added up | A machine is created, or a pending machine is given a larger size |
| `volumes` | Volumes | A volume is created |
| `volumes_gib` | The size of every volume, added up | A volume is created or extended |
| `snapshots` | Volume snapshots | A snapshot is created |
| `backups` | Volume backups | A backup is created |
| `backups_gib` | For every backup, the size of the volume it was taken from | A backup is created |
| `networks`, `subnets`, `routers` | Networks, subnets, routers | One is created |
| `security_groups` | Security groups, without the project's `default` group | A group is created |
| `ports` | Ports, without the ports a router makes for itself | A port is created |
| `floating_ips` | Floating IPs | One is allocated |
| `images` | Image records | An image is created |
| `images_bytes` | The stored bytes of every image | An upload or import has been stored |
| `buckets` | Buckets | A bucket is created |
| `secrets`, `certificates`, `access_keys` | Secrets, certificates, access keys | One is created |

`secrets`, `certificates` and `access_keys` are the only ones with a limit when the project sets none. Their defaults are in `[modules.keep]`; see the [Configuration reference](Configuration-reference).

A system admin sets limits. Anyone with a role in the project can read its limits and usage.

```sh
pi project create research --quota machines=20 --quota vcpus=64
pi quota set research memory_mib=131072 volumes_gib=2000
pi quota set research volumes_gib=unlimited
pi quota show research
```

`pi quota set` changes the limits it names and leaves the others. It refuses a name that is not in the table, so a typing mistake is not stored as a limit nothing checks. `--quota` on `pi project create` and `PUT /identity/projects/{id}` take any name, and the latter replaces the whole map. The API behind `pi quota` is `GET` and `PUT /identity/projects/{id}/quotas`.

Usage is not a counter that could drift. Each module counts its own records when it checks a request, and the count, the check and the write happen one request at a time on the leader, so two requests sent together cannot both take the last unit. A resource stops counting when its delete is accepted, which for machines and volumes is before the host has finished removing it. Lowering a limit below what is used deletes nothing; it only refuses what would add more.

What is not covered:

- Object storage has a limit on buckets and none on bytes.
- A pod is not refused. Its machine counts towards `machines`, `vcpus`, `memory_mib` and `disk_gib` once it exists, but the pod controller does not ask first.
- An image upload is checked after it has been received, because its size is not known before. An upload that does not fit is deleted again.
- Editing a size changes what every machine of that size counts for, without a check.
- The usage that `pi quota show` prints is written when something in the project is created or deleted. A project that existed before an upgrade shows `0` for a resource until then. The check itself always counts.
- During a change of leader, a server that has not yet learned it was replaced still admits requests against its own copy. For that moment each of the two can admit up to the limit.

## Roles

There are three built-in roles.

| Role | Meaning |
| --- | --- |
| `admin` | Full control over the cloud or a project. |
| `member` | Create, change and delete resources within a project. |
| `reader` | Read-only access within a project. |

A role binding gives a principal a role, either in one project or system-wide. See [Identity and access](Identity-and-access) for how bindings become token roles, and for the gap in how `reader` is enforced today.

## Modules

The server is built from modules. Each one owns its resource kinds and is the only code that writes them. Module names follow a workshop theme. API paths and CLI nouns use plain words.

| Module | API prefix | Owns |
| --- | --- | --- |
| `gate` | `/identity` | Principals, projects, roles, role bindings, tokens, API keys, quotas |
| `forge` | `/compute` | Machines (virtual machines, containers, bare metal), sizes, hosts, the scheduler |
| `loom` | `/loom` | Pods, deployments, services, jobs, ingresses, stateful sets, daemon sets, autoscalers |
| `fabric` | `/network` | Networks, subnets, ports, routers, security groups, floating IPs |
| `slab` | `/volume` | Block volumes, snapshots, backups, volume types |
| `mold` | `/image` | Images |
| `silo` | `/silo` | Object storage with an S3 API |
| `shelf` | `/shelf` | Shared filesystems |
| `beacon` | `/dns` | DNS zones and records, and the authoritative DNS server |
| `relay` | `/balancers` | Load balancers |
| `keep` | `/secrets` | Secrets, certificates, the cloud CA, access keys, join tokens |
| `blueprint` | `/blueprint` | Stacks |
| `gauge` | `/gauge` | Metrics, alarms, metering |
| `bench` | `/console` | The `pi` CLI and the web console |
| `anvil` | none | The host agent. It runs inside `pistack agent`, not the server. |

`pi modules` prints the first fourteen rows from a running server.

Two other path groups are not modules: `/rpc` is the agent transport, and `/cluster` is used between control-plane servers.

## Machines, pods and scheduling

A machine has a `kind`: `vm`, `container` or `metal`. All three are placed by the same scheduler in `forge`. It looks at each host's capacity and drivers, subtracts what is already placed, applies the machine's host selector and affinity rules, and picks a host.

A pod in `loom` becomes a machine of kind `container`, so pods and virtual machines draw on the same capacity and attach to the same networks and volumes.

A size is a named amount of vCPUs, memory, disk and GPUs. A new cloud has `s.1x1`, `m.2x4`, `l.4x8` and `xl.8x16`. Admins add more with `pi size create`.

## Leader and reconcilers

In a cluster of servers, one is the leader. Only the leader accepts writes and runs reconcilers. The others replicate its log and answer reads. See [Running a cluster](Running-a-cluster).
