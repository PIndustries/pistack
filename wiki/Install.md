# Install

Today PiStack is installed by building it from source.

## From source

You need Rust 1.85 or newer.

```sh
git clone https://github.com/PIndustries/pistack
cd pistack
cargo build --release
```

This produces two binaries:

| Binary | What it is |
| --- | --- |
| `target/release/pistack` | The server and the host agent. |
| `target/release/pi` | The command-line client. |

Copy both somewhere on your `PATH`. Nothing else is installed: there is no service unit, no package and no default config file.

The server and `pi` build and run on Linux and macOS. On macOS the server is only useful in `--dev` mode. The host agent is Linux only in practice: it reads memory from `/proc/meminfo`, so on other systems it reports a host with no memory and nothing is scheduled on it.

## Release binaries

Not available yet. The release pipeline is being built. This section will list download locations, checksums and supported platforms once the first release exists.

## Container images

Not available yet. This section will name the image and tags once they are published.

## What a host needs

A control-plane server needs nothing beyond the binary and a data directory (`/var/lib/pistack` by default).

A host that runs workloads needs the runtime for the kind of workload, and the tools for the network and storage backends you choose:

| For | Needs on the host |
| --- | --- |
| Virtual machines | `/dev/kvm`, libvirt (`virsh`, `qemu:///system`), `qemu-img`, `cloud-localds` |
| Containers and pods | `youki` |
| Overlay networking | `ip`, `iptables`, `ip6tables`, `dnsmasq`, and `scripts/pistack-overlay.py` installed as `/usr/local/sbin/pistack-overlay` |
| Single-host NAT networking | `ip`, `iptables`, and `scripts/pistack-fip.py` installed as `/usr/local/sbin/pistack-fip` |
| OVN networking | Open vSwitch and `ovn-controller`, set up by you |
| LVM volumes | The LVM tools, a volume group and a thin pool |
| Ceph volumes | `rbd` and a reachable Ceph cluster |

The details are on [Agents and hosts](Agents-and-hosts), [Networking](Networking) and [Storage](Storage).

`scripts/lab-bootstrap.sh --install` installs KVM, libvirt, Open vSwitch, OVN, LVM and Rust on an Ubuntu machine. It was written for the project's lab nodes. Read it before you run it on anything else.

## Check the build

```sh
pistack --version
pi --version
```

Then follow the [Quick start](Quick-start).
