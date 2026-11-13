# Releasing

A release is a build tag. Pushing the tag runs `.github/workflows/release.yml`, which builds everything and publishes it. Nobody builds release files by hand.

The release workflow is separate from CI. It never runs on a pull request, it is not a required check, and nothing in day-to-day development waits for it. If it breaks, fix it when the next release is due.

## Build numbers

A build is numbered by the day it was cut: `<year>.<month>.<day>.<build>`. The year has two digits, the month and the day have no leading zero, and the build counts from 1 each day. The first build on 12 November 2026 is `26.11.12.1`, the second that day is `26.11.12.2`, and the first on 3 January 2027 is `27.1.3.1`.

The tag is the build number and nothing else, with no `v` in front. `pistack --version`, `pi --version`, `pi version`, the API root document and the version an agent reports all print it.

The number is given to the compiler as `PISTACK_BUILD`; the workflow does that. A build without it, such as `cargo build` on a checkout, calls itself `dev`. The version in `Cargo.toml` is there because cargo wants one. Nothing shows it and a release does not change it.

There are no pre-releases. Every build is a release and moves the `latest` image tags.

## Cutting a release

1. Pick the build number. Use today's date in UTC, and for the last part look at the tags that day already has:

   ```sh
   git fetch --tags
   git tag --list "$(date -u +%y.%-m.%-d).*"
   ```

   No output means this is build 1. Otherwise take the highest last part and add one. A number is never used twice: when a build turns out bad, fix it and cut the next one.

2. Tag the commit on `main` and push the tag:

   ```sh
   git switch main && git pull
   git tag -s 26.11.12.1 -m "PiStack 26.11.12.1"
   git push origin 26.11.12.1
   ```

There is nothing to edit or merge first. The workflow checks the tag at its first step and stops when it is not a build number: a `v` in front, a leading zero, a four-digit year, month 13, build 0 or a missing part all fail there. It also checks that the binaries in the tarballs and the images print the tag, so `pistack --version` always matches the release it came from.

## What the pipeline does

- Builds `pistack` and `pi` in release mode for Linux x86_64 and aarch64, each on a runner of that architecture, from a clean tree with `--locked` and the build number in `PISTACK_BUILD`.
- Packs `pistack-VERSION-linux-ARCH.tar.gz` for each: the two binaries, the four host helpers, the systemd units, the environment files, `install.sh`, `LICENSE`, `NOTICE` and `README.md`.
- Unpacks each tarball on the runner, runs `install.sh`, checks the binaries and helpers landed and print the build number, and runs `systemd-analyze verify` on the units.
- Builds the server and agent images for both architectures with the same build number and checks each prints it.
- Pushes the images to `ghcr.io/pindustries/pistack` as multi-arch tags, under the build number and `latest`.
- Creates the GitHub Release for the tag with generated notes, the two tarballs and `SHA256SUMS`.

The release appears last. A failed build leaves no release behind. Re-running the workflow for the same tag replaces the files and the image tags and keeps the release notes.

The tarball binaries are built on Ubuntu 22.04 and need glibc 2.35 or newer.

## Rehearsing

Start the workflow by hand from the Actions tab on any branch (`gh workflow run release.yml --ref my-branch`). It builds the tarballs and the images and runs every check, but publishes nothing: no release, no image push. The build is numbered `dev.` plus the short commit hash, as in `dev.7f39751`. The tarballs are attached to the run as artifacts. Do this after changing the workflow, the `Dockerfile` or anything under `packaging/`.

Starting it by hand on a build tag is the same as pushing that tag.

## Verifying a download

Every release carries `SHA256SUMS`. Download it next to the tarball and run:

```sh
sha256sum -c SHA256SUMS --ignore-missing
```

On macOS use `shasum -a 256 -c SHA256SUMS --ignore-missing`. The line for your tarball must say `OK`.

The checksum shows the file is the one the pipeline uploaded. The release files are not signed yet.

## Container images

One repository, two images:

| Tag | What it is |
| --- | --- |
| `ghcr.io/pindustries/pistack:VERSION`, `:latest` | the control plane, `pistack server` |
| `ghcr.io/pindustries/pistack:VERSION-agent`, `:latest-agent` | the host agent, `pistack agent` |

Both are built for `linux/amd64` and `linux/arm64`. Both also contain `pi`.

The server image is distroless and runs as uid 65532. It keeps its state in the `/var/lib/pistack` volume and serves plain HTTP on 8443; set `PISTACK_TLS_LISTEN` for the HTTPS listener agents use. With no arguments it runs `pistack server --init`, which founds a cluster on an empty volume and is ignored once the volume has a store.

```sh
docker run -d --name pistack -p 8443:8443 -v pistack:/var/lib/pistack ghcr.io/pindustries/pistack
```

It has no sudo and no host helpers. The `hostnat` network backend and NFS shares from `shelf` need `pistack-fip` and `pistack-shelf-helper` on the host, so they do not work from this image. Run the server from the tarball on a host that needs them.

The agent image runs as root and carries the four helpers in `/usr/local/sbin` with `python3`, `iproute2`, `iptables`, `dnsmasq` and `sudo`.

```sh
docker run -d --name pistack-agent ghcr.io/pindustries/pistack:latest-agent agent --join URL --token TOKEN
```

What the agent cannot do in a container:

- Run virtual machines. The image has no libvirt, QEMU or Cloud Hypervisor, and a container has no `/dev/kvm` unless you pass it in.
- Run containers. The image has no youki.
- Build host networks. The overlay and floating IP helpers change bridges, namespaces and iptables rules. In a container they change the container's network, not the host's. `--network host --privileged` lets them reach the host, and at that point the container isolates nothing.
- Use Open vSwitch or OVN. The image has no `ovs-vsctl`.

With no runtime on hand the agent registers the host and uses the mock runtime. That is enough to try the API, the scheduler and `pi` against a cluster of containers. For real workloads install the agent on the host from the tarball.

## First release from a new repository

The first image push creates the `pistack` package under the organisation, private by default. Make it public once, in the package settings on GitHub. Later pushes keep the setting.
