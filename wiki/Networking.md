# Networking

The `fabric` module owns networks, subnets, ports, routers, security groups and floating IPs. Those records are the same whatever carries the packets. What carries them is the fabric backend, chosen for the whole cloud.

## Choosing a backend

Set `PISTACK_FABRIC_BACKEND` on the server.

| Value | Datapath | Hosts |
| --- | --- | --- |
| `mock` | None. Records only. | any |
| `hostnat` | NAT on one host, using the libvirt network. | one |
| `overlay` | VXLAN segments, one router namespace per project router. | many |
| `ovn` | OVN logical switches and routers. | many |

If the variable is not set and no choice was saved earlier, the server uses `mock`. With the mock backend every network call succeeds and nothing is configured on any host. This is the default so that a server on a laptop never touches interfaces. It also means a production server started without the variable looks healthy and moves no packets. Check the start-up log:

```text
INFO pistack::fabricchoice: fabric: no backend set or saved; using the mock fabric
```

The choice is cluster-wide. The leader saves it in the store, and a server started without the variable uses the saved value. An unknown value is logged as an error and ignored.

Only the leader runs the backend.

## Resources

```sh
pi network create app
pi subnet create app4 --network app --cidr 10.1.0.0/24
pi machine create web --image ubuntu-24.04 --size m.2x4 --network app
```

| Resource | Notes |
| --- | --- |
| Network | `--kind overlay` (default) or `--kind provider` with `--physnet` and optionally `--vlan`. `--external` marks a network that routers use as their gateway and that floating IPs come from. `--auto-floating-ip` is described below. |
| Subnet | A CIDR on a network, with a gateway, allocation pools, DNS servers and DHCP on by default. |
| Port | An address on a network, bound to a machine. Created for you when a machine names a network. |
| Router | Connects subnets to each other and to one external network. |
| Security group | Rules by direction, protocol, port range and remote CIDR or group. |
| Floating IP | An address on an external network mapped to a port. |

## Automatic floating IPs

A machine gets no floating IP unless someone asks. Creating an external network changes nothing for the machines that exist.

An admin can mark one external network so that every machine with an IPv4 address gets a floating IP from it:

```sh
pi network create public --external --auto-floating-ip
pi network set public --auto-floating-ip true     # an existing network
pi network set public --auto-floating-ip false
```

- Only an admin can turn it on. Anyone who may edit the network can turn it off.
- Only an external network can carry it, and only one network in the cloud at a time. Marking a second one is refused until the first is turned off.
- It applies to machines in every project. The floating IP is created in the machine's project, named after the machine, labelled `pistack.io/auto-fip=true`, and deleted with the machine.
- A machine labelled `pistack.io/auto-fip=false` gets none: `pi machine create ... --label pistack.io/auto-fip=false`. A project with that label opts out all its machines; a machine in it labelled `pistack.io/auto-fip=true` still gets one. Setting the label later removes the automatic floating IP the machine or project already had.
- It counts against the project's `floating_ips` quota. When the quota is full, or the pool has no free address, the machine keeps running without a floating IP and carries a condition of type `FloatingIp` with the reason (`QuotaExceeded`, `PoolExhausted`) and the message. The condition goes away when the floating IP could be created. `pi machine show <name> -o json` shows it under `status.conditions`.
- Turning the setting off stops new floating IPs. The ones already given stay until their machine is deleted or you delete them with `pi fip delete`.
- A floating IP you create and associate yourself is never touched, and a machine that has one is not given a second.

Upgrading from a build without the setting: until then a network named `fleet`, or the only external network, handed a floating IP to every machine. Those floating IPs stay in place and keep working. No new ones are created until you mark a network. The server logs one warning when it becomes leader while such floating IPs exist and no network is marked. Run `pi fip list` and look for the label to see them.

## Scheduling

A machine that names a network is only scheduled on a host that reported an underlay address. The mock host of `--dev` mode reports none, so in dev mode such a machine stays `pending`. A pod that names no network uses its project's network if there is exactly one; with several you must name one.

## mock

Records every call in memory. Networks reach `ready` and nothing exists on any host. Use it to try the API.

## hostnat

A single-host datapath. The server and the host agent must be the same machine.

Guests sit on the libvirt NAT network `pistack` (`10.88.0.0/24`). The address on the port is the address inside the guest. The server calls a helper to do the privileged work:

- A subnet adds its IPv4 CIDR to the list of guest networks allowed out.
- A floating IP becomes a NAT rule on the underlay interface.
- Security groups become rules in the iptables chain `PISTACK-SG`.

Networks, ports and routers are records only in this backend. Security groups and the guest allow-list are IPv4 only.

On the host:

- `ip` and `iptables`.
- `scripts/pistack-fip.py` installed as `/usr/local/sbin/pistack-fip`, runnable by the server's user through `sudo -n`.
- The helper's settings in its environment. All five are required and have no defaults: `PISTACK_FIP_POOL`, `PISTACK_FIP_GUESTS`, `PISTACK_FIP_SOURCE`, `PISTACK_FIP_IFACE`, `PISTACK_FIP_HOST`. The header of the script explains each. Because the helper runs through `sudo`, sudoers has to let those variables through; the repository does not ship that configuration.
- `scripts/pistack-fip-restore.service` reinstalls the rules at boot.

## overlay

The multi-host datapath that is furthest along.

Each project network is a segment with its own bridge and VXLAN id. Two projects can use the same CIDR. Hosts reach each other over the underlay addresses they reported at join, on UDP port 4789, with a static forwarding table rather than learning.

The server works out a plan per host and sends it to each agent. The agent hands the plan to a helper that makes the host match it:

- a bridge and a VXLAN device per segment,
- a namespace per router, holding the gateway address, SNAT and floating-IP DNAT,
- `dnsmasq` in the router namespace for DHCP, handing each guest the address of its port, the subnet's DNS servers and routes,
- security groups in the iptables chain `PISTACK-OVL`, for IPv4 and IPv6,
- for a provider network, a VLAN sub-interface on the named physical link, or the link itself.

A project router lives on one host: the one with the lowest underlay address. Traffic leaving the project goes through that host.

On every host:

- `ip`, `bridge`, `iptables`, `ip6tables`, `sysctl` and `dnsmasq`.
- `scripts/pistack-overlay.py` installed as `/usr/local/sbin/pistack-overlay`, runnable by the agent's user through `sudo -n`. `PISTACK_OVERLAY_BIN` names another path. If the helper is missing, applying a plan fails.
- An underlay address in a private range. Set `PISTACK_UNDERLAY_IP` if the address the agent picks by itself is wrong.

The helper keeps its state in `/var/lib/pistack/overlay`.

Instance metadata (`169.254.169.254`) is redirected to port 18880 on the host's loopback, where the server's metadata service listens. On a host that does not also run a server nothing listens there, so metadata does not work on agent-only hosts as far as the code shows. The guest is recognised by its source address alone, so two networks with the same subnet are not told apart here; OVN, below, does tell them apart.

## ovn

The server writes OVN's northbound database directly. It does not run `ovn-nbctl`.

| Variable | Meaning |
| --- | --- |
| `PISTACK_OVN_NB` | Northbound database. `unix:/path` or `tcp:host:port`. Default `unix:/var/run/ovn/ovnnb_db.sock`. `ssl:` is not supported. |
| `PISTACK_OVN_GATEWAY_CHASSIS` | Comma-separated chassis names for router gateways, in priority order. |

What the server creates:

- a logical switch per network, with a `localnet` port for provider networks and a `localport` for metadata,
- DHCP options per subnet, with an on-link route to the metadata address,
- a switch port per port, with port security,
- logical routers with a gateway port, a default route and SNAT,
- port groups and ACLs for security groups, with a default drop,
- floating IPs as `dnat_and_snat` rules.

If `PISTACK_OVN_GATEWAY_CHASSIS` is empty, router gateway ports are bound to no chassis. Outbound NAT and floating IPs then do not leave the cluster.

What you have to provide:

- OVN central (northbound and southbound databases, `ovn-northd`) reachable from the leader.
- Open vSwitch and `ovn-controller` on every host, registered as a chassis with its encapsulation address. PiStack does not set this up.

On the host, the agent plugs pod interfaces into `br-int` with `ovs-vsctl`, and libvirt plugs a virtual machine's interface as an Open vSwitch port.

### Metadata on OVN

A guest on an OVN network asks `http://169.254.169.254/` as on the other backends. The path there is different, because OVN gives a guest no way to an address on its host.

- Every logical switch has a port named `pimeta-<network uuid>` of type `localport` that holds `169.254.169.254`. A localport exists on every host, and its traffic never crosses a tunnel, so each host answers the guests it runs.
- DHCP gives the guest an on-link route to `169.254.169.254/32` (option 121 with router `0.0.0.0`). The default route is repeated in that option, because a client that is given option 121 ignores the router option. A pod takes no lease; the agent adds the same route when it plugs the pod.
- On each host, for each network that has a guest there, the agent keeps a network namespace named like the port, a veth pair from `br-int` into it, and a listener on `169.254.169.254:80` inside the namespace. Nothing listens on an address of the host.
- The agent passes the request on to the server over its own connection, the one it uses for everything else. It says which network the namespace belongs to and which address the connection came from. The server answers only when a machine placed on that host holds that address on that network. Nothing the guest sends is trusted: its headers are not read, and only the path is passed on. Two projects with the same subnet get their own answers.
- The agent looks at every heartbeat and before it starts a machine. A network that lost its last guest on the host loses the namespace, the port on `br-int` and the listener. Namespaces left by an agent that was restarted are adopted or removed the same way.
- A security group cannot cut a guest off from metadata: the port group every secured port joins allows TCP port 80 to `169.254.169.254`.

What it needs:

- The agent runs as root, as the packaged unit does. It creates namespaces and opens a socket inside each. An agent that is not root logs once that guests on OVN networks get no metadata from that host.
- A leader. The agent asks the leader, so while the cluster has none the guest gets `503`.
- A guest that was running before the server was upgraded learns the route when it renews its lease, at the latest six hours later, or at once with `networkctl renew <interface>` or a reboot.

A virtual machine still gets its name, keys and user data from the seed disk at first boot and does not depend on this. Metadata is what serves secrets (`/latest/meta-data/secrets/<name>`), the floating IP (`/latest/meta-data/public-ipv4`) and anything in the guest that asks the address.

This path is tested against stand-ins for `ip`, `ovs-vsctl` and the northbound database, not on hosts. To check it on a lab with two OVN hosts, as an admin:

```sh
pi host label <host-a> metacheck=a
pi host label <host-b> metacheck=b
pi network create meta-check
pi subnet create meta-check4 --network meta-check --cidr 10.77.0.0/24
pi machine create mc-a --image ubuntu-24.04 --size s.1x1 --network meta-check \
    --ssh-key @$HOME/.ssh/id_ed25519.pub --host-selector metacheck=a --wait
pi machine create mc-b --image ubuntu-24.04 --size s.1x1 --network meta-check \
    --ssh-key @$HOME/.ssh/id_ed25519.pub --host-selector metacheck=b --wait
```

Use an image and a size you have. Then, on each host, with `<uuid>` the network id from `pi network show meta-check` without its `net-` prefix:

```sh
ip netns list | grep pimeta-<uuid>                      # the namespace is there
ip netns exec pimeta-<uuid> ip -4 addr show meta0       # 169.254.169.254/32
ip netns exec pimeta-<uuid> ip route                    # 10.77.0.0/24 dev meta0
ip netns exec pimeta-<uuid> ss -ltn                     # 169.254.169.254:80
ovs-vsctl --columns=name,external_ids find Interface external_ids:iface-id=pimeta-<uuid>
ovn-sbctl --columns=logical_port,type find Port_Binding logical_port=pimeta-<uuid>   # type localport
ss -ltn | grep 169.254.169.254                          # nothing: the host itself does not listen
```

and in each guest:

```sh
ip route get 169.254.169.254                 # dev <nic>, no "via"
curl -s http://169.254.169.254/latest/meta-data/hostname      # mc-a in mc-a, mc-b in mc-b
curl -s http://169.254.169.254/latest/meta-data/local-ipv4    # the guest's own 10.77.0.x
curl -s http://169.254.169.254/latest/meta-data/public-keys/0/openssh-key   # the key given above
curl -s -H 'X-Forwarded-For: <address of the other guest>' \
    http://169.254.169.254/latest/meta-data/hostname          # still this guest's own name
```

Delete `mc-a`, wait one heartbeat (15 seconds unless configured otherwise), and `ip netns list` on host a no longer shows the namespace while host b still serves `mc-b`. If `ip route get` in the guest shows `via` the gateway, the guest did not take the on-link route; `ip route` and the DHCP client's log say what it was given.

## Load balancers

The `relay` module has its own backend, `PISTACK_RELAY_BACKEND`.

| Value | Meaning |
| --- | --- |
| `mock` | Records only. |
| `forward` | A TCP and UDP forwarder and an HTTP reverse proxy, both inside the server process. |

Unset, it follows the fabric: `forward` when the fabric is `hostnat`, `overlay` or `ovn`, otherwise `mock`. Any other value stops the server at start.

The forwarder runs on the leader, so balancer traffic passes through the leader. Listeners bind loopback unless the balancer's address is a loopback or private address. There is no OVN load balancer.

`pi` has no balancer commands yet. The API is under `/balancers`; use `pi api`. Services and ingresses in `loom` create balancers for you.

## DNS

The `beacon` module runs an authoritative DNS server inside every control-plane server, on UDP and TCP.

Where it listens:

- If `PISTACK_DNS_LISTEN` is set, exactly there. One `host:port`, or several separated by commas.
- Otherwise, on the server's `--listen` address, port 5353, when that address is a private one.
- Otherwise on `127.0.0.1:5353`.

It never binds `0.0.0.0` by default. If the port is taken the server does not start. Port 5353 is the mDNS port, so on macOS and on Linux machines running avahi you must set `PISTACK_DNS_LISTEN`:

```sh
PISTACK_DNS_LISTEN=127.0.0.1:15353 pistack server ...
```

The server answers only for its own zones and refuses everything else. It does not recurse or forward. Record types are A, AAAA, CNAME, MX, TXT, SRV, NS and PTR. Secondary zones are filled by zone transfer over TCP. Dynamic update and NOTIFY are not implemented.

A zone labelled `pistack.io/auto=true` gets an address record for every machine in its project, named `<machine>.<zone>`, using the floating IP when the machine has one.

`pi` has no zone commands yet. The API is under `/dns`.
