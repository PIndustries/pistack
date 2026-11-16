# Install

PiStack is installed from a release tarball, or built from source.

## Release binaries

Releases for Linux x86_64 and aarch64 are on the [releases page](https://github.com/PIndustries/pistack/releases). Each tarball is `pistack-VERSION-linux-ARCH.tar.gz` and holds:

| Path | What it is |
| --- | --- |
| `bin/pistack` | The server and the host agent. |
| `bin/pi` | The command-line client. |
| `scripts/` | The host helpers: `pistack-overlay`, `pistack-fip`, `pistack-shelf-helper`, `pistack-youki-cleanup`. |
| `systemd/` | `pistack-server.service` and `pistack-agent.service`. |
| `env/` | `server.env` and `agent.env`, the settings the units read. |
| `install.sh` | Installs all of the above. |

Every release carries `SHA256SUMS`. Check the download against it before you run anything; [docs/RELEASING.md](../docs/RELEASING.md) shows how.

The binaries are built on Ubuntu 22.04 and need glibc 2.35 or newer, so they run on Linux only. To install from an unpacked tarball:

```sh
sudo ./install.sh
```

It puts the binaries in `/usr/local/bin`, the helpers in `/usr/local/sbin`, the units in `/etc/systemd/system` and the settings in `/etc/pistack`. It starts nothing and enables nothing. Both units run as root, and the helpers need `python3` and `sudo`. [Quick start](Quick-start) and [Running a cluster](Running-a-cluster) cover the settings and starting the services.

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

Copy both somewhere on your `PATH`. `sudo packaging/install.sh` from a checkout installs the binaries, the helpers and the units the same way the tarball does. A build from source needs no settings file: pass flags or set the environment.

The server and `pi` build and run on Linux and macOS. On macOS the server is only useful in `--dev` mode. The host agent is Linux only in practice: it reads memory from `/proc/meminfo`, so on other systems it reports a host with no memory and nothing is scheduled on it.

## Container images

Not published yet. A release builds a server image (`pistack server`) and an agent image (`pistack agent`) for `linux/amd64` and `linux/arm64`, but no image is published until the first release that pushes them. [docs/RELEASING.md](../docs/RELEASING.md) describes the images and their limits; this section will name the repository and tags once they exist.

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
