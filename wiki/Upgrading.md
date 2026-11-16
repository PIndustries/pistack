# Upgrading

How to move a running PiStack from one build to the next, and what changes with each.

Versions are written `year.month.day.build`, as in `26.11.13.1`: a two-digit year, month and day with no leading zero, and a build that counts from 1 each day. The build number is what `pistack --version` and `pi --version` print, and it is the release tag. [docs/RELEASING.md](../docs/RELEASING.md) explains how a build is cut.

## Before any upgrade

Take a backup of the store first. A migrated store cannot be opened by an older binary, so a backup is the only way back. See [Running a cluster](Running-a-cluster#backup).

The store records a schema version and every server checks it on its own copy when it opens. Nothing here raises it, so these upgrades replace the binary in place; see [Running a cluster](Running-a-cluster#upgrades).

## From 26.11.13.1 to the next build

Replace the binary on every server, one at a time, followers first. The steps below apply to the server's own binary and settings; an agent's binary is covered in [Agents and hosts](Agents-and-hosts).

### A new server now refuses `--join http://...` alone

A server joining for the first time downloads a snapshot of the whole store, secrets included, and sends the join token with the request. A brand-new server has no store, so it has no CA of the cloud's to check the leader with. From this build on it refuses to start that download on its own:

| `--join` address | `--join-ca-sha256` set | What happens |
| --- | --- | --- |
| `https://` | yes | The leader must present a certificate that chains to that CA. |
| `https://` | no | Refused before anything is sent. |
| `http://` | no | Refused. |
| `http://` | yes | Refused: a fingerprint cannot be checked over plain HTTP. |

Pass one of these instead:

```sh
# over TLS: the leader's --tls-listen address and the cloud's root CA fingerprint
PISTACK_JOIN_TOKEN=pij_... PISTACK_JOIN_CA_SHA256=<hex> \
  pistack server --join https://10.0.0.11:8444 --listen 10.0.0.12:8443

# over plain HTTP, on a network you trust
PISTACK_JOIN_TOKEN=pij_... \
  pistack server --join http://10.0.0.11:8443 --insecure-join --listen 10.0.0.12:8443
```

`pi join-token create` prints the fingerprint under the token. It is not a secret and is the same for every token of one cloud.

Servers that already hold a store are unaffected: a member checks the others against the CA in its own store. Discovery has no TLS address to hand out, so a server that joins that way also needs `--insecure-join`. The flag and the fingerprint matter for the first start only. [Adding servers](Running-a-cluster#adding-servers) has the full table.

### Upgrade every server before the first signing-key rotation

This build gives token signing keys ids, so they can be rotated (`pi signing-key rotate`). Until the first rotation both builds sign with the same secret and each accepts the other's tokens, so they run side by side during a rolling upgrade. Run the first `rotate` only once every server is on the new build. A server still on 26.11.13.1 after a rotation:

- refuses tokens signed with the new key (`401 invalid token`), because it knows only the old secret;
- when the first key is retired, by hand or by a later `rotate` once its 7 days pass, loses the secret the servers recognise each other with. It can no longer be recognised by the others, and a restart writes a new secret into its own copy of the store.

That key the servers recognise each other with is now its own value in the store and does not change with a rotation, so servers on either build agree on it until the old secret is deleted. [Identity and access](Identity-and-access#upgrading-from-a-build-without-key-ids) gives the details.

### A token for a project the caller has no role in is refused

Before this build, logging in scoped to any project returned a token with no roles, and a caller with no roles could read. Anyone with a password could read the machines, networks, images and volumes of any project by logging in scoped to it.

Now a token is issued for a project only when the principal holds a role bound in it, or a role at system scope. Asking for a project the principal has no role in is a `403`. The same check runs when a token is used, so a token stops working once the last role in its project is removed. A login with no project asked for, or with a default project the principal has no role in, comes back with no project rather than being refused.

Before upgrading, grant a role to every account that logs in scoped to a project:

```sh
pi binding create --principal alice --role member --in-project research
```

After the upgrade an account with no role anywhere cannot get a scoped token: an admin grants one with `pi binding create`, or the login asks for no project.

### A server that cannot follow the log now heals itself

A member the leader can no longer bring up to date, because it fell behind what the leader keeps or because it once led and holds entries no majority confirmed, used to stop and wait for an operator to move its store aside and rejoin it. It now downloads a fresh snapshot, moves the old store to `state.redb.replaced-<time>`, and runs its own binary again. It keeps its id and its role. Two things to know:

- It replaces its store at most once every ten minutes; the wait is not configurable.
- The entries it held alone are set aside, not merged. They stay in `state.redb.replaced-<time>`.

The old manual step still works and is what to do if you turn this off with `PISTACK_AUTO_RESYNC=0`. See [When a server cannot follow the log](Running-a-cluster#when-a-server-cannot-follow-the-log).

### Automatic floating IPs are off until you mark a network

An automatic floating IP used to be given to every machine with an IPv4, from the external network named `fleet` or the only external one. That meant creating an external network put an outside address on every machine. It is now opt-in: only an external network an admin has marked hands one out, one network for the whole cloud.

```sh
pi network set <EXTERNAL-NETWORK> --auto-floating-ip true
```

After an upgrade, floating IPs handed out before stay; none are added until a network is marked, and the leader logs one warning about it. A machine labelled `pistack.io/auto-fip=false` gets none, and a machine on a network whose floating-IP quota is full shows the reason on its `FloatingIp` condition instead of only in the log. See [Networking](Networking).

### A browser on a follower can write when it asks

A write sent to a server that does not lead is refused with `409` and the leader's address, as before. A client can now set `X-PiStack-Forward: leader` to have a follower carry the request to the leader and return the answer; the web console does this on every request, so the console on any server works. Nothing changes without the header: `pi` and other clients still get the `409`. Bodies over 8 MiB are not carried, and the leader's audit record shows the carrying server's address, not the client's.
