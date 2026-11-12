# Quick start

This starts one server in dev mode and creates a machine with `pi`. It takes a few minutes and runs on Linux or macOS. Nothing here needs root.

Dev mode registers a mock host inside the server. Machines are scheduled and report `ready`, but nothing boots. It is for trying the API and the CLI. To run real machines you need a Linux host with an agent; see [Agents and hosts](Agents-and-hosts).

## Build

```sh
git clone https://github.com/PIndustries/pistack
cd pistack
cargo build
```

## Start the server

```sh
PISTACK_DNS_LISTEN=127.0.0.1:15353 \
PISTACK_DATA_DIR=/tmp/pistack-dev \
target/debug/pistack server --init --dev --listen 127.0.0.1:18443 --insecure-http
```

What the parts do:

- `--init` founds a new cluster in an empty data directory.
- `--dev` adds the mock host and treats requests without a token as admin.
- `--insecure-http` says you know this listener is plain HTTP.
- `PISTACK_DNS_LISTEN` moves the built-in DNS server off port 5353. On macOS and on Linux machines that run avahi, 5353 is taken by mDNS and the server will not start without this.

On first start the server generates an admin password and writes it to `admin-password` in the data directory, readable only by the user the server runs as:

```text
WARN gate: bootstrapped admin with a generated password; read it from the file, change it, delete the file principal="admin" file=/var/lib/pistack/admin-password
```

The password is not in the log. To choose it yourself, set `PISTACK_ADMIN_PASSWORD` before the first start; then no file is written.

## Look around

In another terminal, from the same directory:

```sh
alias pi="$PWD/target/debug/pi --url http://127.0.0.1:18443"

pi version
pi modules
pi size list
pi host list
```

`pi host list` shows one host named `local` with the driver `mock`. `pi size list` shows the four sizes the server creates on first start: `s.1x1`, `m.2x4`, `l.4x8` and `xl.8x16`.

## Create a machine

```sh
pi machine create web --image ubuntu-24.04 --size m.2x4 --wait
pi machine list
pi machine show web
```

```text
machine "web" created (mach-69655f47)
machine "web": pending
machine "web": ready
```

The mock host does not check that the image exists, so any image name works here. On a real host the image must be in the image store first; see [Storage](Storage).

Stop, start and delete it:

```sh
pi machine stop web --wait
pi machine start web --wait
pi machine delete web --yes --wait
```

## Create resources from a file

`pi apply` reads YAML or JSON. Each document has a `kind` (the CLI noun), a `name` and a `spec`.

```yaml
kind: network
name: app
spec:
  mtu: 1450
---
kind: machine
name: api
labels:
  tier: backend
spec:
  kind: vm
  size: s.1x1
  image: ubuntu-24.04
```

```sh
pi apply -f app.yaml
```

Running it again updates the resources in place.

## Log in

Without a token, dev mode makes every request an admin with no project. That is enough for the steps above. To work as a real principal, log in:

```sh
pi login http://127.0.0.1:18443 --name admin
pi whoami
```

`pi login` asks for the password and saves a token in a profile, so later commands need no `--url`. The profile file is `~/.config/pistack/config.toml` on Linux and `~/Library/Application Support/pistack/config.toml` on macOS.

One thing to know before you mix the two: a resource created without a token belongs to no project. A token is scoped to a project, so after you log in, `pi machine list` does not show the machines you created before logging in. Pick one way of working per session.

## Clean up

Stop the server with Ctrl-C and remove the data directory:

```sh
rm -rf /tmp/pistack-dev
```

## Next

- [Concepts](Concepts) explains what you just created.
- [Running a cluster](Running-a-cluster) covers more than one server.
- [Known limits](Known-limits) lists what does not work yet.
