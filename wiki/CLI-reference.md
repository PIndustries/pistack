# CLI reference

`pi` is the PiStack command-line client. This page lists every command as of `main`. It was generated from the output of `pi <command> -h`, so the descriptions are the ones the tool prints.

```text
pi [OPTIONS] <COMMAND>
```

Most commands are a noun and a verb: `pi machine create`, `pi volume list`. Wherever a command takes a `<NAME>`, it also takes the resource's id.

## Global options

These work on every command.

| Option | Environment | Meaning |
| --- | --- | --- |
| `--config <FILE>` | `PI_CONFIG` | Profile file. Default `~/.config/pistack/config.toml` on Linux, `~/Library/Application Support/pistack/config.toml` on macOS. |
| `--profile <NAME>` | `PI_PROFILE` | Profile to use. Default: the one `pi profile use` picked. |
| `--url <URL>` | `PI_URL` | API URL. Overrides the profile. |
| `--token <TOKEN>` | `PI_TOKEN` | Bearer token. Overrides the profile. |
| `--project <PROJECT>` | `PI_PROJECT` | Project scope for `pi login`. Overrides the profile. |
| `--insecure` | | Skip TLS certificate verification. |
| `-o, --output <OUTPUT>` | | `table` (default), `wide`, `json`, `yaml` or `name`. |
| `--no-headers` | | Omit the header row in table output. |
| `-h, --help` | | Print help. |
| `-V, --version` | | Print the client version. |

## Verbs most nouns share

| Verb | Options | Meaning |
| --- | --- | --- |
| `list` | `--all`, `-l, --selector <K=V>` | List. `--all` asks for every project (admin). `-l` filters by label and can be repeated. |
| `show <NAME>` | | Show one resource. Use `-o yaml` or `-o json` for the full record. |
| `create <NAME>` | per noun, below | Create. |
| `delete <NAME>` | `-y, --yes`, `--wait` | Delete. Asks for confirmation unless `--yes`. `--wait` returns when the resource is gone. |

The tables below list only the arguments and options that are specific to a command. An empty meaning is an option the tool prints no help text for.

## General commands

| Command | What it does |
| --- | --- |
| `pi login` | Log in with a password and store the token in a profile |
| `pi whoami` | Show who the current token is |
| `pi modules` | List the modules the server has mounted |
| `pi version` | Print client and server versions |
| `pi api` | Make a raw API request: `pi api GET /compute/machines` |
| `pi apply` | Create or update resources from a YAML/JSON manifest |
| `pi completion` | Shell completion script |

```text
pi login --name <NAME> <URL>
pi whoami
pi modules
pi version
pi api <METHOD> <PATH>
pi apply --file <FILES>
pi completion <SHELL>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `login` | `<URL>` | API URL, e.g. https://cloud.example:8443 |
| `login` | `--name <NAME>` | Principal name |
| `login` | `--password-stdin` | Read the password from stdin instead of prompting |
| `login` | `--scope-project <PROJECT>` | Project to scope the token to (name or id) |
| `login` | `--ttl <TTL>` | Token lifetime in seconds |
| `api` | `<METHOD>` | HTTP method |
| `api` | `<PATH>` | Path (e.g. /compute/machines) or full URL |
| `api` | `-d, --data <DATA>` | Request body: inline JSON or @file |
| `apply` | `-f, --file <FILES>` | Manifest file (YAML or JSON, multi-document ok); `-` for stdin |
| `completion` | `<SHELL>` | [possible values: bash, zsh, fish, elvish, powershell] |

## pi profile

Manage named connection profiles.

```text
pi profile list
pi profile use <NAME>
pi profile show [NAME]
pi profile delete <NAME>
```

## pi project

Projects (tenants).

```text
pi project list
pi project show <NAME>
pi project create <NAME>
pi project delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `project create` | `--description <DESCRIPTION>` |  |
| `project create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `project create` | `--parent <PARENT>` | Parent project (name or id) |
| `project create` | `--quota <RES=N>` | Quota `resource=limit` (repeatable) |
| `project create` | `--disabled` |  |

## pi quota

Project quotas: limits and usage.

```text
pi quota show [PROJECT]
pi quota set <PROJECT> <RES=N>...
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `quota show` | `[PROJECT]` | Project (name or id); default: the current scope |
| `quota set` | `<PROJECT>` | Project (name or id) |
| `quota set` | `<RES=N>...` | Limit `resource=N`; `resource=unlimited` removes it (repeatable) |

## pi user

Users and service accounts.

```text
pi user list
pi user show <NAME>
pi user create <NAME>
pi user delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `user create` | `--description <DESCRIPTION>` |  |
| `user create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `user create` | `--kind <KIND>` | [default: user] [possible values: user, service-account] |
| `user create` | `--email <EMAIL>` |  |
| `user create` | `--password <PASSWORD>` | Password (prefer --password-stdin) |
| `user create` | `--password-stdin` | Read the password from stdin |
| `user create` | `--default-project <DEFAULT_PROJECT>` | Default project (name or id) |
| `user create` | `--disabled` |  |

## pi role

Roles.

```text
pi role list
pi role show <NAME>
pi role create <NAME>
pi role delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `role create` | `--description <DESCRIPTION>` |  |

## pi binding

Role bindings.

```text
pi binding list
pi binding show <NAME>
pi binding create --principal <PRINCIPAL> --role <ROLE>
pi binding delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `binding create` | `--principal <PRINCIPAL>` | Principal (name or id) |
| `binding create` | `--role <ROLE>` | Role name |
| `binding create` | `--in-project <PROJECT>` | Project (name or id); omit for system scope |

## pi key

API keys.

```text
pi key list
pi key show <NAME>
pi key create <NAME>
pi key delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `key create` | `--principal <PRINCIPAL>` | Principal (name or id); default: you |
| `key create` | `--scope-project <PROJECT>` | Project the key's tokens are scoped to (name or id) |
| `key create` | `--expires-at <EXPIRES_AT>` | Expiry, RFC 3339 |

## pi access-key

Access keys for request signing.

```text
pi access-key list
pi access-key show <NAME>
pi access-key create
pi access-key delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `access-key create` | `--description <DESCRIPTION>` | Human-readable note on the key |
| `access-key create` | `--principal <PRINCIPAL>` | Principal the key acts as (name or id); default: you |

## pi join-token

Join tokens for `pistack agent --join` (admin).

```text
pi join-token list
pi join-token show <NAME>
pi join-token create <NAME>
pi join-token delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `join-token create` | `--multi-use` | Allow more than one redemption |
| `join-token create` | `--max-uses <N>` | Max redemptions for a multi-use token (implies --multi-use). Default and maximum come from the server (typically 100) |
| `join-token create` | `--ttl-secs <SECS>` | Lifetime in seconds (default and maximum 86400 = 24h) |

## pi machine

Machines: VMs, containers, bare metal.

```text
pi machine list
pi machine show <NAME>
pi machine create --size <SIZE> --image <IMAGE> <NAME>
pi machine delete <NAME>
pi machine start <NAME>
pi machine stop <NAME>
pi machine reboot <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `machine create` | `--description <DESCRIPTION>` |  |
| `machine create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `machine create` | `--size <SIZE>` | Size name, e.g. m.2x4 |
| `machine create` | `--image <IMAGE>` | Image name |
| `machine create` | `--network <NETWORK>` | Network (repeatable) |
| `machine create` | `--volume <VOLUME>` | Volume (repeatable) |
| `machine create` | `--near-volume <VOLUME>` | Place the machine where this volume can attach (repeatable). Nothing is attached; use `pi volume attach` once it runs |
| `machine create` | `--kind <KIND>` | [default: vm] [possible values: vm, container, metal] |
| `machine create` | `--ssh-key <KEY>` | SSH public key (repeatable); `@file` reads a file |
| `machine create` | `--user-data <USER_DATA>` | cloud-init user data; `@file` reads a file |
| `machine create` | `--host-selector <K=V>` | Host selector `key=value` (repeatable) |
| `machine create` | `--zone <ZONE>` | Availability zone: only hosts labelled `zone=<ZONE>` |
| `machine create` | `--wait` | Wait until the machine is ready (or failed) |
| `machine start` | `--wait` |  |
| `machine stop` | `--wait` |  |
| `machine reboot` | `--wait` |  |

## pi pod

Pods (loom).

```text
pi pod list
pi pod show <NAME>
pi pod create --image <IMAGE> <NAME>
pi pod delete <NAME>
pi pod exec <POD> <COMMAND>...
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `pod create` | `--description <DESCRIPTION>` |  |
| `pod create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `pod create` | `--image <IMAGE>` | Container image |
| `pod create` | `--container <CONTAINER>` | Container name (default: the pod name) |
| `pod create` | `--network <NETWORKS>` | Network to attach (repeatable). Empty means the project's default |
| `pod exec` | `<POD>` | Pod name or id |
| `pod exec` | `<COMMAND>...` | Command and arguments. Flags after `--` belong to the command |
| `pod exec` | `-c, --container <CONTAINER>` | Container name. Required when the pod has more than one |
| `pod exec` | `--timeout <TIMEOUT>` | Seconds to wait. The server rejects values outside 1..=3600 [default: 30] |

## pi size

Sizes (flavors).

```text
pi size list
pi size show <NAME>
pi size create --vcpus <VCPUS> --memory <MIB> <NAME>
pi size delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `size create` | `--description <DESCRIPTION>` |  |
| `size create` | `--vcpus <VCPUS>` |  |
| `size create` | `--memory <MIB>` | Memory in MiB |
| `size create` | `--disk <GIB>` | Root disk in GiB [default: 0] |
| `size create` | `--gpus <GPUS>` | [default: 0] |

## pi host

Hosts (admin).

```text
pi host list
pi host show <NAME>
pi host label <NAME> [K=V]...
pi host disable <NAME>
pi host enable <NAME>
pi host drain <NAME>
pi host evacuate <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `host label` | `[K=V]...` | Labels to set, `key=value` |
| `host label` | `--remove <KEY>` | Label key to remove (repeatable) |
| `host label` | `--replace` | Drop every label that is not given here |
| `host disable` | `--reason <REASON>` | Why, shown by `pi host show` |
| `host drain` | `--reason <REASON>` | Why, shown by `pi host show` |
| `host evacuate` | `--rebuild-from-image` | Accept that every evacuated machine loses its root disk and starts again from its image. Without it no machine is moved |
| `host evacuate` | `--confirm-dead` | The host is disabled but may still be reporting, and you know it is dead. If it is not, its machines will run twice until it reports |

`host disable` stops new placements; the machines on the host keep running. `host drain` disables the host and live-migrates what it can. `host evacuate` is for a host that is down. See [Agents and hosts](Agents-and-hosts#host-maintenance).

## pi cluster

The control-plane servers (admin).

```text
pi cluster status
```

`status` asks the server `pi` is pointed at and prints each server's id, its role, which one leads, and the addresses clients and agents use for it.

## pi server

Take a control-plane server out of the cluster (admin). `pi cluster status` lists the servers.

```text
pi server remove <SERVER>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `server remove` | `<SERVER>` | Server id, the start of one, or its API address |
| `server remove` | `--force` | Remove it even if the servers left could not form a majority |
| `server remove` | `-y, --yes` | Do not ask for confirmation |

## pi network

Networks.

```text
pi network list
pi network show <NAME>
pi network create <NAME>
pi network delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `network create` | `--description <DESCRIPTION>` |  |
| `network create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `network create` | `--kind <KIND>` | [default: overlay] [possible values: overlay, provider] |
| `network create` | `--vlan <VLAN>` | VLAN tag (provider networks) |
| `network create` | `--physnet <PHYSNET>` | Physical network name (provider networks) [default: physnet1] |
| `network create` | `--mtu <MTU>` | [default: 1500] |
| `network create` | `--external` | External network: routers get gateways and floating IPs here |
| `network create` | `--dns-domain <DNS_DOMAIN>` |  |

## pi subnet

Subnets.

```text
pi subnet list
pi subnet show <NAME>
pi subnet create --network <NETWORK> --cidr <CIDR> <NAME>
pi subnet delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `subnet create` | `--description <DESCRIPTION>` |  |
| `subnet create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `subnet create` | `--network <NETWORK>` | Network (name or id) |
| `subnet create` | `--cidr <CIDR>` | CIDR, e.g. 10.1.0.0/24 |
| `subnet create` | `--gateway <GATEWAY>` |  |
| `subnet create` | `--dns <IP>` | DNS nameserver (repeatable) |
| `subnet create` | `--pool <START-END>` | Allocation pool `start-end` (repeatable) |
| `subnet create` | `--no-dhcp` |  |

## pi port

Ports.

```text
pi port list
pi port show <NAME>
pi port create --network <NETWORK> <NAME>
pi port delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `port create` | `--description <DESCRIPTION>` |  |
| `port create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `port create` | `--network <NETWORK>` | Network (name or id) |
| `port create` | `--subnet <SUBNET>` | Subnet (name or id) |
| `port create` | `--ip <IP>` | Fixed IP (repeatable) |
| `port create` | `--mac <MAC>` |  |
| `port create` | `--secgroup <SG>` | Security group (repeatable, name or id) |
| `port create` | `--machine <MACHINE>` | Machine to plug into (name or id) |
| `port create` | `--no-port-security` |  |

## pi router

Routers.

```text
pi router list
pi router show <NAME>
pi router create <NAME>
pi router delete <NAME>
pi router set-interfaces <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `router create` | `--description <DESCRIPTION>` |  |
| `router create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `router create` | `--external-network <EXTERNAL_NETWORK>` | External network for the gateway (name or id) |
| `router create` | `--subnet <SUBNET>` | Subnet to attach (repeatable, name or id) |
| `router set-interfaces` | `--subnet <SUBNET>` | Subnet (repeatable, name or id). Replaces the current set |

## pi secgroup

Security groups.

```text
pi secgroup list
pi secgroup show <NAME>
pi secgroup create <NAME>
pi secgroup delete <NAME>
pi secgroup add-rule --direction <DIRECTION> <NAME>
pi secgroup remove-rule <NAME> <RULE>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `secgroup create` | `--description <DESCRIPTION>` |  |
| `secgroup create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `secgroup add-rule` | `--direction <DIRECTION>` | [possible values: ingress, egress] |
| `secgroup add-rule` | `--proto <PROTO>` | [default: any] [possible values: tcp, udp, icmp, any] |
| `secgroup add-rule` | `--port <PORT>` | Port or range: `22` or `8000-9000` |
| `secgroup add-rule` | `--cidr <CIDR>` | Remote CIDR |
| `secgroup add-rule` | `--remote-group <REMOTE_GROUP>` | Remote security group (name or id) |
| `secgroup add-rule` | `--ethertype <ETHERTYPE>` | [default: ipv4] [possible values: ipv4, ipv6] |
| `secgroup remove-rule` | `<RULE>` | Rule id (see `pi secgroup show`) |

## pi fip

Floating IPs.

```text
pi fip list
pi fip show <NAME>
pi fip create --network <NETWORK> <NAME>
pi fip delete <NAME>
pi fip associate <NAME>
pi fip disassociate <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `fip create` | `--description <DESCRIPTION>` |  |
| `fip create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `fip create` | `--network <NETWORK>` | External network (name or id) |
| `fip create` | `--address <ADDRESS>` | Specific address (default: allocate) |
| `fip create` | `--port <PORT>` | Port to associate at once (name or id) |
| `fip associate` | `--port <PORT>` | Port (name or id) |
| `fip associate` | `--machine <MACHINE>` | Machine (name or id). Uses that guest's current IPv4 |

## pi volume

Block volumes.

```text
pi volume list
pi volume show <NAME>
pi volume create <NAME>
pi volume delete <NAME>
pi volume attach --machine <MACHINE> <NAME>
pi volume detach <NAME>
pi volume extend --size <GIB> <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `volume create` | `--description <DESCRIPTION>` |  |
| `volume create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `volume create` | `--size <GIB>` | Size in GiB. With `--image` and no size, the image virtual size is rounded up to the next GiB |
| `volume create` | `--type <VOLUME_TYPE>` | Volume type [default: standard] |
| `volume create` | `--image <IMAGE>` | Populate from an image (name or id) |
| `volume create` | `--snapshot <SNAPSHOT>` | Restore from a snapshot (name or id) |
| `volume create` | `--source-volume <SOURCE_VOLUME>` | Clone another volume (name or id) |
| `volume create` | `--bootable` |  |
| `volume create` | `--encrypted` |  |
| `volume create` | `--wait` |  |
| `volume attach` | `--machine <MACHINE>` | Machine (name or id) |
| `volume attach` | `--device <DEVICE>` | Guest device name, e.g. vdb |
| `volume extend` | `--size <GIB>` | New size in GiB |

## pi snapshot

Volume snapshots.

```text
pi snapshot list
pi snapshot show <NAME>
pi snapshot create --volume <VOLUME> <NAME>
pi snapshot delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `snapshot create` | `--description <DESCRIPTION>` |  |
| `snapshot create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `snapshot create` | `--volume <VOLUME>` | Volume (name or id) |

## pi voltype

Volume types.

```text
pi voltype list
pi voltype show <NAME>
pi voltype create <NAME>
pi voltype delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `voltype create` | `--description <DESCRIPTION>` |  |
| `voltype create` | `--backend <BACKEND>` | Backend: local, lvm, rbd [default: local] |
| `voltype create` | `--param <K=V>` | Backend parameter `key=value` (repeatable) |

## pi image

Images.

```text
pi image list
pi image show <NAME>
pi image create --format <FORMAT> <NAME>
pi image delete <NAME>
pi image upload --file <FILE> <NAME>
pi image download <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `image create` | `--description <DESCRIPTION>` |  |
| `image create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `image create` | `--format <FORMAT>` | [possible values: raw, qcow2, oci] |
| `image create` | `--os <OS>` |  |
| `image create` | `--arch <ARCH>` | [default: x86_64] |
| `image create` | `--min-disk <GIB>` | [default: 0] |
| `image create` | `--min-ram <MIB>` | [default: 0] |
| `image create` | `--visibility <VISIBILITY>` | [default: private] [possible values: private, project, public] |
| `image create` | `--source-url <SOURCE_URL>` | Import from a URL after creating |
| `image create` | `--file <FILE>` | Upload this file after creating |
| `image upload` | `--file <FILE>` |  |
| `image download` | `-O, --out <FILE>` | Output file (default: `<name>.<format>`) |

## pi deployment

Deployments (loom).

```text
pi deployment list
pi deployment show <NAME>
pi deployment create --image <IMAGE> <NAME>
pi deployment delete <NAME>
pi deployment scale --replicas <REPLICAS> <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `deployment create` | `--description <DESCRIPTION>` |  |
| `deployment create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `deployment create` | `--image <IMAGE>` | Container image |
| `deployment create` | `--container <CONTAINER>` | Container name (default: the Deployment name) |
| `deployment create` | `--replicas <REPLICAS>` | Desired replicas [default: 1] |
| `deployment scale` | `--replicas <REPLICAS>` |  |

## pi service

Services (loom).

```text
pi service list
pi service show <NAME>
pi service create --port <PORT> <NAME>
pi service delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `service create` | `--description <DESCRIPTION>` |  |
| `service create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `service create` | `--kind <KIND>` | Service kind [default: cluster-ip] [possible values: cluster-ip, node-port, load-balancer] |
| `service create` | `--port <PORT>` | Port to expose |
| `service create` | `--selector <K=V>` | Selector `key=value` (optional; both halves required when set) |

## pi job

Jobs (loom).

```text
pi job list
pi job show <NAME>
pi job create --image <IMAGE> <NAME>
pi job delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `job create` | `--description <DESCRIPTION>` |  |
| `job create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `job create` | `--image <IMAGE>` | Container image |
| `job create` | `--container <CONTAINER>` | Container name (default: the Job name) |
| `job create` | `--completions <COMPLETIONS>` | Successful completions required [default: 1] |
| `job create` | `--parallelism <PARALLELISM>` | Max pods running at once [default: 1] |
| `job create` | `--restart <RESTART>` | Pod restart policy (Never or OnFailure) [default: never] [possible values: never, on-failure] |

## pi ingress

Ingresses (loom).

```text
pi ingress list
pi ingress show <NAME>
pi ingress create --service <SERVICE> <NAME>
pi ingress delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `ingress create` | `--description <DESCRIPTION>` |  |
| `ingress create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `ingress create` | `--host <HOST>` | Optional host for the rule |
| `ingress create` | `--path <PATH>` | Path (must start with /) [default: /] |
| `ingress create` | `--service <SERVICE>` | Backend service name |
| `ingress create` | `--port <PORT>` | Backend service port [default: 80] |

## pi statefulset

StatefulSets (loom).

```text
pi statefulset list
pi statefulset show <NAME>
pi statefulset create --image <IMAGE> <NAME>
pi statefulset delete <NAME>
pi statefulset scale --replicas <REPLICAS> <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `statefulset create` | `--description <DESCRIPTION>` |  |
| `statefulset create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `statefulset create` | `--image <IMAGE>` | Container image |
| `statefulset create` | `--container <CONTAINER>` | Container name (default: the StatefulSet name) |
| `statefulset create` | `--replicas <REPLICAS>` | Desired replicas [default: 1] |
| `statefulset create` | `--service-name <SERVICE_NAME>` | Service that gives pods a stable network identity |
| `statefulset scale` | `--replicas <REPLICAS>` |  |

## pi daemonset

DaemonSets (loom).

```text
pi daemonset list
pi daemonset show <NAME>
pi daemonset create --image <IMAGE> <NAME>
pi daemonset delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `daemonset create` | `--description <DESCRIPTION>` |  |
| `daemonset create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `daemonset create` | `--image <IMAGE>` | Container image |
| `daemonset create` | `--container <CONTAINER>` | Container name (default: the DaemonSet name) |

## pi autoscaler

Autoscalers (loom).

```text
pi autoscaler list
pi autoscaler show <NAME>
pi autoscaler create --target-kind <TARGET_KIND> --target <TARGET> --metric <METRIC> --target-value <TARGET_VALUE> --max <MAX_REPLICAS> <NAME>
pi autoscaler delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `autoscaler create` | `--description <DESCRIPTION>` |  |
| `autoscaler create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `autoscaler create` | `--target-kind <TARGET_KIND>` | Workload kind to scale [possible values: deployment, stateful-set] |
| `autoscaler create` | `--target <TARGET>` | Target Deployment or StatefulSet (name or id) |
| `autoscaler create` | `--metric <METRIC>` | Gauge metric name |
| `autoscaler create` | `--target-value <TARGET_VALUE>` | Target metric value |
| `autoscaler create` | `--min <MIN_REPLICAS>` | Minimum replicas [default: 1] |
| `autoscaler create` | `--max <MAX_REPLICAS>` | Maximum replicas |

## pi stack

Stacks (blueprint).

```text
pi stack list
pi stack show <NAME>
pi stack create --document <DOCUMENT> <NAME>
pi stack delete <NAME>
pi stack update --document <DOCUMENT> <NAME>
pi stack apply <NAME>
pi stack rollback <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `stack create` | `--description <DESCRIPTION>` |  |
| `stack create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `stack create` | `-d, --document <DOCUMENT>` | Stack document (YAML/JSON text); `@file` reads a file, `-` reads stdin |
| `stack update` | `-d, --document <DOCUMENT>` | Replacement document; `@file` or `-` for stdin |
| `stack rollback` | `--revision <REVISION>` | Revision from status.history (default: previous) |

## Not in the CLI yet

These parts of the API have no `pi` commands. Use `pi api` with the paths shown.

| What | Path |
| --- | --- |
| DNS zones and records | `/dns` |
| Load balancers | `/balancers` |
| Secrets, certificates, certificate authorities | `/secrets` |
| Buckets | `/silo/buckets` |
| Shares | `/shelf` |
| Volume backups | `/volume/backups` |
| Metrics and alarms | `/gauge` |

```sh
pi api GET /dns/zones
pi api POST /silo/buckets -d '{"name":"backups"}'
```
