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
pi project mine
pi project switch <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `project create` | `--description <DESCRIPTION>` |  |
| `project create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `project create` | `--parent <PARENT>` | Parent project (name or id) |
| `project create` | `--quota <RES=N>` | Quota `resource=limit` (repeatable) |
| `project create` | `--disabled` |  |
| `project switch` | `<NAME>` | Project name or id to move to |

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

## pi alarm

Threshold alarms on metrics (gauge). See [Metrics and alarms](Metrics-and-alarms).

```text
pi alarm list
pi alarm show <NAME>
pi alarm create --metric <SERIES> --comparison <COMPARISON> --threshold <VALUE> <NAME>
pi alarm update <NAME>
pi alarm delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `alarm create` | `--description <DESCRIPTION>` |  |
| `alarm create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `alarm create` | `--metric <SERIES>` | Series to watch: a name, or a name and labels such as `pistack_host_load1{host="h1"}`. With labels the alarm watches every series that has them; with none, every series of that name |
| `alarm create` | `--comparison <COMPARISON>` | Which side of the threshold fires [possible values: gt, lt, eq] |
| `alarm create` | `--threshold <VALUE>` |  |
| `alarm create` | `--for-seconds <SECONDS>` | How long the value must keep crossing before it fires, in seconds. The wait is measured on the stamps of the samples themselves [default: 0] |
| `alarm show` | `--samples` | Also print the points the deciding series has now |
| `alarm update` | `--description <DESCRIPTION>` |  |
| `alarm update` | `--label <K=V>` | Set a label `key=value` (repeatable) |
| `alarm update` | `--metric <SERIES>` | Series to watch |
| `alarm update` | `--comparison <COMPARISON>` | [possible values: gt, lt, eq] |
| `alarm update` | `--threshold <VALUE>` |  |
| `alarm update` | `--for-seconds <SECONDS>` | How long the value must keep crossing before it fires, in seconds |

`show` prints the record with its `status`: the phase, the latest value the phase was decided from, and the series that decided it. With `--samples` the points held for that series follow; for `-o json|yaml` they ride inside the record under `samples`. `update` changes what you name and keeps the rest; a new spec is decided again at once from the window, so a new threshold shows without waiting for the next sample, and its hold begins from the stamps there.

## pi metric

The series one server holds points for (gauge). There is no history: what a server holds is the last minutes of each series, kept in memory. See [Metrics and alarms](Metrics-and-alarms).

```text
pi metric list
pi metric show <SERIES>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `metric list` | `--name <NAME>` | Keep series with this metric name |
| `metric show` | `<SERIES>` | The series, as `pi metric list` prints it: `queue.depth`, or `pistack_host_load1{host="h1"}` |
| `metric show` | `--last <N>` | Keep the last N points |

`list` prints one row per series with its newest point, and `show` the points a server holds for one series, oldest first. The window is the server answering the request: behind a load balancer, one row can come from one server and the next from another.

## pi audit

The audit log (admin).

```text
pi audit list
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `audit list` | `--since <TIME>` | Records from this time on: an RFC 3339 time, or an age such as 30m, 2h or 7d |
| `audit list` | `--until <TIME>` | Records before this time, given like --since |
| `audit list` | `--principal <WHO>` | Principal name or id |
| `audit list` | `--in-project <PROJECT>` | Project name or id of the token that made the request |
| `audit list` | `--action <ACTION>` | Action: create, update, delete, login, drain, update_quotas, ... |
| `audit list` | `--kind <KIND>` | Resource kind as in the path: machines, bindings, hosts, ... |
| `audit list` | `--resource <ID>` | Resource id or name, as the request gave it |
| `audit list` | `--outcome <OUTCOME>` | `success`, `failure`, or one status code such as 403 |
| `audit list` | `--carried-by <NODE>` | Records a server of the cluster carried to the leader, by its node id |
| `audit list` | `--limit <LIMIT>` | Most records to print [default: 100] |
| `audit list` | `--after <ID>` | Continue after this record id (the last one of the page before) |
| `audit list` | `--oldest-first` | Oldest first instead of newest first |

Records print newest first. The filter is `--in-project` because `--project` is the global option that sets the scope of the request. `-o wide` adds the project, the API key, the server that carried the request, the method, the path and the record id; the carried-by column is empty when the request reached the leader itself. See [Identity and access](Identity-and-access#audit) for what a record holds.

## pi user

Users and service accounts.

```text
pi user list
pi user show <NAME>
pi user create <NAME>
pi user delete <NAME>
pi user revoke-tokens <NAME>
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

`user revoke-tokens` refuses every token the user holds at that moment. They log in again to get a new one. An admin can do it for anyone, a user for themselves. See [Identity and access](Identity-and-access#tokens).

## pi signing-key

The keys tokens are signed with (admin). `signing-keys` is an alias.

```text
pi signing-key list
pi signing-key rotate
pi signing-key retire
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `signing-key retire` | `-y, --yes` | Do not ask for confirmation |

`list` prints each key's id, whether it signs new tokens, its age and when it stops verifying. `rotate` signs with a new key and lets the one before it verify for 7 more days. `retire` drops every key but the current one at once, which logs out everyone whose token was signed with an older key. See [Identity and access](Identity-and-access#rotating-the-signing-key).

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

`create` prints the token once, and under it `ca sha256`, the fingerprint of the cloud's root CA. A new control-plane server is given both; see [Running a cluster](Running-a-cluster#adding-servers). An agent needs only the token.

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
pi machine rebuild <NAME>
pi machine resize <NAME> --size <SIZE>
pi machine stats <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `machine create` | `--description <DESCRIPTION>` |  |
| `machine create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `machine create` | `--size <SIZE>` | Size name, e.g. m.2x4 |
| `machine create` | `--image <IMAGE>` | Image name |
| `machine create` | `--network <NETWORK>` | Network (repeatable) |
| `machine create` | `--volume <VOLUME>` | Volume (repeatable) |
| `machine create` | `--kind <KIND>` | [default: vm] [possible values: vm, container, metal] |
| `machine create` | `--ssh-key <KEY>` | SSH public key (repeatable); `@file` reads a file |
| `machine create` | `--keypair <KEYPAIR>` | Key pair (name or id) whose public key the machine also gets |
| `machine create` | `--user-data <USER_DATA>` | cloud-init user data; `@file` reads a file |
| `machine create` | `--host-selector <K=V>` | Host selector `key=value` (repeatable) |
| `machine create` | `--zone <ZONE>` | Availability zone: only hosts labelled `zone=<ZONE>` |
| `machine create` | `--wait` | Wait until the machine is ready (or failed) |
| `machine start` | `--wait` |  |
| `machine stop` | `--wait` |  |
| `machine reboot` | `--wait` |  |
| `machine rebuild` | `--image <IMAGE>` | Image the new root disk is a copy of (name or id). Default: the image the machine has |
| `machine rebuild` | `-y, --yes` | Do not ask for confirmation |
| `machine rebuild` | `--wait` | Wait until the machine is ready again (or failed) |
| `machine resize` | `--size <SIZE>` | New size name or id |

`machine resize` changes the size of a machine. It must be stopped first; a running one is refused. The new size must be one of the sizes the project may use. A larger root disk takes effect on the next start.

`machine stats` prints the current value of each of the machine's usage series, and for a counter how fast it is rising. A machine that is off, or one whose runtime does not measure, has none. See [Metrics and alarms](Metrics-and-alarms).

`machine rebuild` replaces the machine's root disk with a fresh copy of an image and starts it again. The machine keeps its id, name, host, ports, addresses, volumes, keys and size; what was on the root disk is gone, and first-boot setup runs again. It works on a machine that is ready or stopped, and the machine goes to `updating` and back to `ready` with the power state it had. It asks for confirmation unless `--yes`.

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

## pi keypair

SSH key pairs.

```text
pi keypair list
pi keypair show <NAME>
pi keypair create [--public-key-file <FILE>] <NAME>
pi keypair delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `keypair create` | `--description <DESCRIPTION>` |  |
| `keypair create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `keypair create` | `--public-key-file <FILE>` | Import this public key file instead of making a key |
| `keypair create` | `--out <FILE>` | Write a generated private key here, mode 0600, never overwritten |

A create with `--public-key-file` imports the key in it. A create without it makes an ed25519 pair: the server keeps the public half and returns the private half once, and the CLI writes it to `--out` or sends it to a pipe. It is never printed on a terminal. The private half is not stored anywhere, so a pair made this way is lost if you do not keep the file. The name is settled before the server is asked: an existing file, or a directory that cannot be written, stops the command with nothing created. If the file still fails to be written after the pair is made, the pair is deleted again.

`--keypair` on `machine create` gives the machine that pair's public key, beside any `--ssh-key`. The machine keeps the name, which `pi machine show` prints as `spec.keypair`, and it cannot be changed afterwards. The pair can be deleted afterwards; the machine keeps the key it was given.

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
pi host stats <NAME>
pi host label <NAME> [K=V]...
pi host disable <NAME>
pi host enable <NAME>
pi host drain <NAME>
pi host evacuate <NAME>
pi host power set <NAME> --kind <KIND> --address <ADDR> --user <USER> --secret <SECRET>
pi host power show <NAME>
pi host power clear <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `host label` | `[K=V]...` | Labels to set, `key=value` |
| `host label` | `--remove <KEY>` | Label key to remove (repeatable) |
| `host label` | `--replace` | Drop every label that is not given here |
| `host disable` | `--reason <REASON>` | Why, shown by `pi host show` |
| `host drain` | `--reason <REASON>` | Why, shown by `pi host show` |
| `host evacuate` | `--rebuild-from-image` | Accept that every evacuated machine loses its root disk and starts again from its image. Without it no machine is moved |
| `host evacuate` | `--confirm-dead` | Say the host is dead by your word. Needed for a host with no power control, every time; optional for one the server can power off |
| `host power set` | `--kind <KIND>` | `ipmi` or `redfish` |
| `host power set` | `--address <ADDR>` | BMC address: a `host[:port]` for IPMI, a URL or host for Redfish |
| `host power set` | `--user <USER>` | BMC user |
| `host power set` | `--secret <SECRET>` | Name of the secret holding the BMC password. The password itself is never stored, shown or logged |

`host stats` prints what the host is using now: load, memory, its data disk and how many machines it holds. See [Metrics and alarms](Metrics-and-alarms).

`host list -o wide` adds `CERT-EXPIRES`, the last day of the host's agent certificate, and `host show` prints it as `status.cert_expires`. It is empty for a host that never enrolled.

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
pi network set <NAME> --auto-floating-ip <BOOL>
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
| `network create` | `--auto-floating-ip` | Give every machine a floating IP from this network without being asked. Admin only; needs `--external`; one network at a time |
| `network create` | `--dns-domain <DNS_DOMAIN>` |  |
| `network set` | `<NAME>` | Name or id |
| `network set` | `--auto-floating-ip <BOOL>` | Turn automatic floating IPs from this external network on or off. Turning it off leaves the floating IPs already given in place [possible values: true, false] |

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

## pi backup

Volume backups. A backup is one volume's bytes stored in a bucket. The bucket has to exist already.

```text
pi backup list
pi backup show <NAME>
pi backup create --volume <VOLUME> --bucket <BUCKET> <NAME>
pi backup delete <NAME>
pi backup restore <BACKUP>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `backup create` | `--description <DESCRIPTION>` |  |
| `backup create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `backup create` | `--volume <VOLUME>` | Volume to back up (name or id) |
| `backup create` | `--bucket <BUCKET>` | Bucket to write the backup to (name or id) |
| `backup create` | `--wait` | Wait until the backup is ready |
| `backup restore` | `--volume <VOLUME>` | Write into this volume (name or id) instead of making a new one. It must be ready and at least as large as the backup |

A new backup is `pending` while its bytes are uploaded and `ready` once they are stored. `pi backup create --wait` prints each phase and ends on `ready`; a failed upload ends with the reason. A restore needs a `ready` backup and returns once the volume's bytes match the stored checksum. Without `--volume` it makes a new volume named after the backup, in the source volume's type and size.

## pi bucket

Object storage buckets.

```text
pi bucket list
pi bucket show <NAME>
pi bucket create <NAME>
pi bucket delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `bucket create` | `--description <DESCRIPTION>` |  |
| `bucket create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `bucket create` | `--versioning` | Keep the previous bytes when a key is written again |

## pi object

Objects in a bucket. A get without `--out` writes the bytes to stdout; a put streams the file up.

```text
pi object list <BUCKET> [--prefix <PREFIX>]
pi object put <BUCKET> <KEY> <FILE>
pi object get <BUCKET> <KEY> [--out <FILE>]
pi object delete <BUCKET> <KEY>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `object list` | `--prefix <PREFIX>` | Only keys starting with this |
| `object get` | `--out <FILE>` | Write to this file instead of stdout |
| `object delete` | `-y, --yes` | Do not ask for confirmation |


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

## pi lb

Load balancers, and the listeners, pools and members under them. A balancer owns one address (a VIP) on a network; listeners bind a protocol and port on it and point at a pool; a pool holds backends, one `address:port` each.

```text
pi lb list
pi lb show <NAME>
pi lb create --network <NETWORK> <NAME>
pi lb delete <NAME>
pi lb listener list
pi lb listener show <NAME>
pi lb listener create --balancer <BALANCER> --port <PORT> <NAME>
pi lb listener delete <NAME>
pi lb pool list
pi lb pool show <NAME>
pi lb pool create --balancer <BALANCER> <NAME>
pi lb pool delete <NAME>
pi lb member list
pi lb member add --pool <POOL> --port <PORT> <ADDRESS>
pi lb member remove --pool <POOL> <MEMBER>
```

Deleting a balancer deletes its listeners, pools and members with it. A pool a listener points at cannot be deleted until the listener is changed or gone.

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `lb create` | `--description <DESCRIPTION>` |  |
| `lb create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `lb create` | `--network <NETWORK>` | Network (name or id) the VIP lives on |
| `lb create` | `--vip <VIP>` | Explicit VIP address (default: allocated) |
| `lb create` | `--vip-port <VIP_PORT>` | Bind this exact port on the VIP instead of an ephemeral one |
| `lb create` | `--provider <PROVIDER>` | Data plane [default: forward] [possible values: forward, relay] |
| `lb create` | `--down` | Bring it up administratively off |
| `lb listener list` | `--balancer <BALANCER>` | Filter to one balancer (name or id) |
| `lb listener create` | `--balancer <BALANCER>` | Balancer (name or id) the listener binds on |
| `lb listener create` | `--protocol <PROTOCOL>` | [default: tcp] [possible values: tcp, udp, http, https, terminated] |
| `lb listener create` | `--port <PORT>` |  |
| `lb listener create` | `--pool <POOL>` | Default pool (name or id) |
| `lb listener create` | `--cert <CERT>` | Certificate id for `--protocol terminated` |
| `lb listener create` | `--connection-limit <CONNECTION_LIMIT>` | Max concurrent connections; 0 means unlimited [default: 0] |
| `lb listener create` | `--down` | Bring it up administratively off |
| `lb pool list` | `--balancer <BALANCER>` | Filter to one balancer (name or id) |
| `lb pool create` | `--balancer <BALANCER>` | Balancer (name or id) the pool belongs to |
| `lb pool create` | `--protocol <PROTOCOL>` | [default: tcp] [possible values: tcp, udp, http, https] |
| `lb pool create` | `--algorithm <ALGORITHM>` | [default: round_robin] [possible values: round_robin, least_conn, source_ip] |
| `lb pool create` | `--health-check` | Add a health monitor with these probe settings |
| `lb pool create` | `--monitor <MONITOR>` | What the monitor speaks. Default: the pool protocol; a udp pool is probed with a bare connect [default: tcp] [possible values: tcp, http, https] |
| `lb pool create` | `--interval <INTERVAL>` | Seconds between probes [default: 5] |
| `lb pool create` | `--timeout <TIMEOUT>` | Seconds before a probe fails [default: 3] |
| `lb pool create` | `--rise <RISE>` | Consecutive successes before a member is online [default: 2] |
| `lb pool create` | `--fall <FALL>` | Consecutive failures before a member is offline [default: 3] |
| `lb pool create` | `--down` | Bring it up administratively off |
| `lb member list` | `--pool <POOL>` | Filter to one pool (name or id) |
| `lb member list` | `--only <ONLY>` | Members whose name or address contains this |
| `lb member add` | `--pool <POOL>` | Pool (name or id) to add the backend to |
| `lb member add` | `--port <PORT>` | Backend port |
| `lb member add` | `--weight <WEIGHT>` | Share of traffic; 0-256 [default: 1] |
| `lb member add` | `--backup` | Take no new traffic while others are up |
| `lb member add` | `--down` | Add it administratively off |
| `lb member remove` | `--pool <POOL>` | Pool (name or id) |
| `lb member remove` | `<MEMBER>` | Member name, id, `ADDRESS:PORT`, or address alone when the pool holds one member on it. A member's name is made from its pool, address and port; see `pi lb member list` |

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

## pi share

Shared filesystems (shelf). A share is exported over `nfs` or mounted as `cephfs`, and carries the list of who may mount it.

```text
pi share list
pi share show <NAME>
pi share create <NAME>
pi share delete <NAME>
pi share access list <NAME>
pi share access allow --cidr <CIDR> <NAME>
pi share access deny --cidr <CIDR> <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `share create` | `--description <DESCRIPTION>` |  |
| `share create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `share create` | `--protocol <PROTOCOL>` | Protocol: nfs or cephfs [default: nfs] |
| `share create` | `--size <GIB>` | Size in GiB [default: 10] |
| `share create` | `--cidr <CIDR>` | Allow a client CIDR to mount (repeatable) |
| `share create` | `--mode <MODE>` | Mode for the CIDRs given: read or write [default: write] |
| `share access allow` | `--cidr <CIDR>` | Client CIDR, for example 10.0.0.0/24 |
| `share access allow` | `--mode <MODE>` | Mount mode for the rule [default: write] |
| `share access deny` | `--cidr <CIDR>` | Client CIDR, for example 10.0.0.0/24 |

`access list` prints the client CIDR rules. An NFS share with no CIDR rule stays pending: there is nothing to export until a client is named. A rule naming a principal rather than a CIDR is not shown here; `pi share show` lists the whole record.

The API has no endpoint for one rule, so `allow` and `deny` read the share, edit its access list and send the whole spec back with `If-Match` set to the revision they read. If the share was written in between, the command reports a revision mismatch and changes nothing; run it again.

## pi secret

Secrets (keep). The value of a secret never appears on the command line: it comes from a file or from stdin, and only `secret get` prints it back.

```text
pi secret list
pi secret show <NAME>
pi secret create <NAME>
pi secret delete <NAME>
pi secret get <NAME>
pi secret put <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `secret create` | `--description <DESCRIPTION>` |  |
| `secret create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `secret create` | `--value-file <PATH>` | The value: a file, or `-` for stdin. Default: stdin |
| `secret create` | `--kind <KIND>` | [default: opaque] [possible values: opaque, password, ssh-key, token, certificate] |
| `secret create` | `--content-type <CONTENT_TYPE>` | MIME type of the value. Default depends on the kind |
| `secret create` | `--expires-at <EXPIRES_AT>` | When the value stops being valid, RFC 3339 |
| `secret create` | `--rotate-after-days <DAYS>` | Warn once the value is this many days old |
| `secret get` | `--version <N>` | The version to print. Only the current payload is kept, so this checks the version that comes back and fails if it is another one |
| `secret put` | `--value-file <PATH>` | The new value: a file, or `-` for stdin. Default: stdin |

A trailing newline is trimmed off whatever is read: a pipe and an editor end a file with one, and it is not part of the secret. An empty value is refused. `list`, `show` and `apply` carry the record, never the payload; a manifest that names a secret which does not exist yet prints what to run instead of creating a record with nothing in it.

## pi cert

Certificates issued by the cloud CA (keep).

```text
pi cert list
pi cert show <NAME>
pi cert issue <NAME>
pi cert delete <NAME>
pi cert revoke <NAME>
pi cert key <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `cert issue` | `--description <DESCRIPTION>` |  |
| `cert issue` | `--label <K=V>` | Label `key=value` (repeatable) |
| `cert issue` | `--common-name <COMMON_NAME>` | Subject of the certificate. Defaults to the name |
| `cert issue` | `--san <NAME>` | DNS name or address (repeatable). The subject is not added to it |
| `cert issue` | `--usage <USAGE>` | [default: server] [possible values: server, client, both] |
| `cert issue` | `--ttl-days <DAYS>` | Days until it expires. The issuing CA can cut this short |
| `cert issue` | `--issuer <ISSUER>` | Certificate authority to sign with (id); default: the issuing CA. Naming one needs admin |

`show` includes the certificate PEM; `key` prints the certificate, its private key and the chain, and needs admin. `revoke` is not a deletion — the record stays so its serial stays in the CRL. Certificate authorities have no noun of their own; `pi api GET /secrets/cas` lists them.

## pi dns zone

DNS zones. A zone is an apex, such as `example.cloud.`, that this cloud is authoritative for. `show` takes the resource name, its id, or the apex itself.

```text
pi dns zone list
pi dns zone show <NAME>
pi dns zone create <APEX>
pi dns zone delete <NAME>
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `zone create` | `<APEX>` | Apex of the zone, e.g. example.cloud or example.cloud. A missing final dot is added |
| `zone create` | `--name <NAME>` | Name the zone is filed under. Default: the apex without its final dot |
| `zone create` | `--description <DESCRIPTION>` |  |
| `zone create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `zone create` | `--ttl <TTL>` | Default TTL for records that do not set one, in seconds [default: 3600] |
| `zone create` | `--email <EMAIL>` | Contact mailbox for the SOA, e.g. hostmaster@example.cloud |
| `zone create` | `--nameserver <NAMESERVER>` | Authoritative nameserver (repeatable, absolute name). Default: `ns1.<apex>` |
| `zone create` | `--also-notify <ADDR>` | Address of an outside secondary of this zone (`ip` or `ip:port`, repeatable), told when the serial moves. A primary zone only |
| `zone create` | `--primary <ADDR>` | Primary to transfer the zone from (`ip` or `ip:port`, repeatable). Makes the zone a secondary |

A zone made with `--primary` is a secondary: it is served only once a zone transfer has filled it and stays `pending` until then, and it takes no record sets — the module refuses those with `record sets belong on primary zones`. `--primary` and `--also-notify` contradict each other and clap refuses the pair. `pi dns zone list -o wide` shows the kind, the primaries a secondary pulls from, and the addresses in `also_notify`.

A field you do not give is left out of the request rather than sent empty, so the module's own default applies and appears in what you read back: no `--email` gives the SOA mailbox `hostmaster`, no `--nameserver` makes the zone authoritative for `ns1.<apex>`. Both are the module's defaults, and the module does not repeat a default back to you, so those wide columns are blank until you set them explicitly.

Deleting a zone deletes every record set in it; the module does not refuse a zone that still has record sets, so `--yes` is the only check on that. A zone labelled `pistack.io/auto=true` also gets machine records written for you; see [Networking](Networking).

## pi dns record

DNS record sets. A record set is one owner name and one type inside a zone, holding one or more records.

```text
pi dns record list
pi dns record show <NAME> [--zone <ZONE>]
pi dns record create <OWNER> <TYPE> --zone <ZONE> --record <DATA>
pi dns record delete <NAME> [--zone <ZONE>]
```

| Command | Argument or option | Meaning |
| --- | --- | --- |
| `record list` | `--zone <ZONE>` | Only this zone's record sets (name, id or apex) |
| `record show` | `--zone <ZONE>` | Look for it in this zone only (name, id or apex) |
| `record delete` | `--zone <ZONE>` | Look for it in this zone only (name, id or apex) |
| `record create` | `<OWNER>` | Owner name, relative to the zone (`web`), absolute with a final dot, or `@` for the apex |
| `record create` | `<TYPE>` | Record type: A, AAAA, CNAME, MX, TXT, SRV, NS or PTR |
| `record create` | `--name <NAME>` | Name the record set is filed under. Default: `<owner>-<type>` lower-cased, with the zone after it when that name is already taken in the project |
| `record create` | `--description <DESCRIPTION>` |  |
| `record create` | `--label <K=V>` | Label `key=value` (repeatable) |
| `record create` | `--zone <ZONE>` | Zone the record lives in (name, id or apex) |
| `record create` | `--ttl <TTL>` | TTL in seconds. Default: the zone's |
| `record create` | `--record <DATA>` | Record data (repeatable), presentation format: `10.0.0.4`, `10 mail.example.cloud.`, `"v=spf1 -all"` |

```sh
pi dns zone create example.cloud --email ops@example.cloud
pi dns record create web A --zone example.cloud --record 10.0.0.4 --record 10.0.0.5
pi dns record create @ TXT --zone example.cloud --record 'v=spf1 -all'
pi dns record list --zone example.cloud
```

`show` and `delete` take `--zone` for the same thing: the set is looked up among that zone's own, so its short name — `<owner>-<type>` — finds it there even when the name it is filed under carries the zone as well. Without `--zone` a record set is read by the name it is filed under, and nothing else.

A name is unique in a project, not in a zone, so two zones holding the same host cannot both be filed as `www-a`: the first takes the short name and the second gets the zone after it, `www-a.corp.cloud`. The record itself is unchanged by either; only the name it is filed under moves.

One owner name holds one set per type, and a CNAME owner holds nothing else. The owner, the type and the zone of an existing set cannot be changed; delete the set and create it again. A set made without `--ttl` carries no TTL of its own and is served with its zone's; `list` and `show` print that number rather than a blank, while `-o json|yaml` pass the server's row through with the TTL still absent.

## Not in the CLI yet

These parts of the API have no `pi` commands. Use `pi api` with the paths shown.

| What | Path |
| --- | --- |
| Certificate authorities | `/secrets/cas` |
| Access keys | `/secrets/access-keys` |
| Join tokens | `/secrets/join-tokens` |
| Posted samples | `/gauge/samples` |

```sh
pi api GET /balancers/balancers
pi api POST /silo/buckets -d '{"name":"backups"}'
```

The zone export — a whole zone in master-file form — has no verb of its own either: `pi api GET /dns/zones/<NAME>/export`.
