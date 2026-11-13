# Running a cluster

A control plane is one or more `pistack server` processes that share a replicated state store. One server is already a cluster of one. Adding servers gives you copies of the state and automatic leader election. Hosts that run workloads are separate; see [Agents and hosts](Agents-and-hosts).

## Starting the first server

```sh
pistack --data-dir /var/lib/pistack server --init --listen 10.0.0.11:8443
```

`--init` founds a new cluster. The server creates `state.redb` in the data directory, becomes the only voter and the leader, and creates the `admin` project and principal. The admin password is taken from `PISTACK_ADMIN_PASSWORD` if set. Otherwise one is generated and written to `admin-password` in the data directory, mode 0600.

After the first start, `--init` is no longer needed. A server that finds an existing `state.redb` stays in that cluster whatever flags it is given.

### TLS

`--listen` always serves plain HTTP. If you do not pass `--insecure-http`, the server warns about it at start; the flag only acknowledges it and makes the default public URL `http://`.

`--tls-listen ADDR` adds an HTTPS listener next to the plain one. The address must be specific, not `0.0.0.0`. The certificate is issued by the cloud CA and kept in `<data_dir>/tls`. Clients do not have that CA in their trust store, so `pi` needs `--insecure` to use it today.

Each server writes the address of its HTTPS listener into its member record, as `agent_api`. That is the address agents on HTTPS are given for it, in the list of servers and when a follower names the leader. `--agent-url` overrides it. A server restarted with a different `--tls-listen` has the record corrected within a few seconds, and one restarted without it has the address removed.

Until this is finished, put the API on a private network.

## Adding servers

```sh
pi join-token create server-2
PISTACK_JOIN_TOKEN=pij_... pistack --data-dir /var/lib/pistack server --join http://10.0.0.11:8443 --listen 10.0.0.12:8443
```

A new server needs a join token, in `PISTACK_JOIN_TOKEN`, to download the snapshot and to join. It needs it only for that first start: once it holds the store it is a member, and the token can expire. A server found by discovery needs the token too.

`--join URL` names any reachable address of the current leader. The new server downloads a snapshot of the store, registers as a member, and starts replicating. Only the leader serves joins; another server answers `409`.

With neither `--init` nor `--join`, a server with an empty data directory tries discovery, but only when `--listen` is a private IPv4 address. It probes every address in its /24 on UDP port 18445. If no cluster answers it founds one. If exactly one answers it joins it. If more than one answers it stops and asks for `--join`. With `--listen 0.0.0.0:8443` (the default) there is no discovery and the server exits, asking for `--init` or `--join`.

Cluster traffic between servers uses the same API port, under `/cluster`. Those routes hand out the whole store, so they answer only a member or a server holding a valid join token. Members prove themselves with a key derived from the token signing secret, which is in every member's store and in every snapshot; nothing has to be handed to them and it does not expire. Each server also states its id on these calls, which is how one that was removed is refused. A server that is refused says so and names `PISTACK_JOIN_TOKEN`.

`PISTACK_REQUIRE_JOIN=0` opens the routes to anything that reaches the port. Servers older than this check send no key, so replacing them one at a time stops replication between old and new; set `PISTACK_REQUIRE_JOIN=0` on every server for that one upgrade, then take it out.

`GET /cluster` is the one cluster route that also answers a cloud admin's token, so you can see what a server believes. `pi cluster status` prints it:

```sh
pi --url http://10.0.0.11:8443 cluster status
```

```text
SERVER                                ROLE   LEADER  API                    AGENT API
019a7c1e-5f0a-7d31-9c55-1b0e8f6a2d10  voter  yes     http://10.0.0.11:8443  https://10.0.0.11:8444
019a7c1e-9a44-7b02-8e17-6c3d2a9f4b71  voter          http://10.0.0.12:8443  https://10.0.0.12:8444
019a7c1f-02be-7a90-b6d3-0f4e7c8a1e52  voter          http://10.0.0.13:8443  https://10.0.0.13:8444
```

The answer is that one server's view. Ask each server when they might disagree. `AGENT API` is empty for a server started without `--tls-listen`.

`pi cluster status -o json`, or `pi api GET /cluster`, prints the whole answer. With `curl`, send an admin token:

```sh
curl -s -H "Authorization: Bearer $PI_TOKEN" http://10.0.0.11:8443/cluster
```

```json
{"cluster_id":"...","node_id":"...","api":"http://10.0.0.11:8443",
 "leader_api":"http://10.0.0.11:8443","leader_id":"...","role":"Voter","leader":true,
 "members":[{"id":"...","api":"http://10.0.0.11:8443","role":"Voter","agent_api":"https://10.0.0.11:8444"},
            {"id":"...","api":"http://10.0.0.12:8443","role":"Voter","agent_api":"https://10.0.0.12:8444"},
            {"id":"...","api":"http://10.0.0.13:8443","role":"Voter","agent_api":"https://10.0.0.13:8444"}],
 "term":1,"commit":74}
```

Without a token the answer is `401`, and a token that is not a cloud admin's gets `403`.

## Learners and voters

Every server joins as a learner. A learner replicates the log but does not vote and cannot become leader.

- With two servers you have one voter and one learner. If the voter stops, nothing takes over.
- When a third server joins and both learners have caught up, the leader promotes them. You then have three voters.
- A fourth server and any after it stay learners. The voter count stays at three.

A learner counts as caught up when it pulled from the leader in the last three seconds and was at most 64 log entries behind.

So the useful sizes are one server, or three or more.

## The leader

Only the leader:

- accepts writes,
- runs reconcilers and controllers,
- serves joins and snapshots,
- promotes learners.

Any server answers reads. A write sent to a server that is not the leader is not forwarded. It is refused with `409` and the leader's address:

```json
{"error":{"code":"conflict","message":"not the leader","details":{"leader":"http://10.0.0.12:8443"}}}
```

`pi` follows that address once: it sends the same request, with the same token, to the leader it was told about and says so on stderr.

```text
http://10.0.0.11:8443 is not the leader; sending the request to http://10.0.0.12:8443
```

It does this only when the leader's address has the same scheme as the one `pi` was given, so a token sent over `https` is never repeated over `http`; otherwise it prints the refusal. It does not remember the leader. To save the extra request, point `pi` at the leader (`pi cluster status` shows which server that is). Other clients have to follow the address themselves, or go through a proxy that sends traffic to whichever server reports `"leader": true` on `/cluster`. The proxy's check needs an admin token.

Agents do follow. An agent that loses its server tries the other servers it knows and registers with the new leader. An agent that joined over HTTPS only ever moves to another HTTPS address: it logs and skips a plain one, so every server it should fail over to needs `--tls-listen`.

### Durability of a write

Today the leader commits a write to its own store and answers the client. Followers pull the entry afterwards, normally within a fraction of a second. If the leader's disk is lost before a follower has pulled, that write is gone. Pull request [PIndustries/pistack#103](https://github.com/PIndustries/pistack/pull/103) changes this so that a write in a cluster with more than one voter waits for a majority of voters. It is not merged yet.

## Failover

When the leader stops, the remaining voters notice within an election timeout (0.8 to 2 seconds, randomised) and elect one of themselves. A candidate first asks for a pre-vote and only raises the term when a majority agrees, so a server that merely lost its network for a moment does not disturb a healthy leader. A voter that heard from the leader in the last 800 ms refuses to vote.

A leader steps down by itself when a voter reports a newer term, or when it has not heard from a majority of voters for two seconds. After stepping down it refuses writes.

In a test with three local servers, stopping the leader gave a new leader within five seconds, and writes to the new leader worked straight away.

During failover:

- Writes fail until a leader is elected, then must go to the new leader.
- Reads keep working on any server that is up.
- Running machines are not touched. Hosts keep running what they have.
- Reconcilers stop on the old leader and start on the new one.

A restarted server finds its existing store and comes back as a follower. It asks the other voters who leads and catches up from the log. No flags are needed beyond the usual `--listen`.

With three voters the cluster keeps a leader with one server down. With two down, the remaining server cannot win an election, and a leader that is left alone steps down. There is no leader and no writes until a second voter returns.

## When a server cannot follow the log

There are two cases in which the leader cannot bring a member up to date from its log, however often the member asks.

- **The member fell too far behind.** The leader keeps the last 10,000 entries, plus whatever a follower seen in the last ten minutes still needs, up to 200,000. A server that was down for longer needs entries the leader has trimmed.
- **The member's log differs from the leader's.** The server was the leader, wrote entries that no majority confirmed, and lost the leadership before they reached anyone. The new leader's log has other entries at those positions. Clients were never told those writes had succeeded: each was answered `503`.

In both cases the member replaces its own store. It downloads a snapshot from the leader it is following, checks it, moves `state.redb` aside as `state.redb.replaced-<time>`, puts the snapshot in its place and starts again. It logs this:

```text
replication stopped: the leader cannot feed this store from its log. This server will replace its store with a snapshot from the leader and start again
replacing this server's store with a snapshot from the leader
store replaced: log entries 31 to 35 of the old store were never confirmed by a majority and are not in the new one. The old store is kept. Starting again on the new store
starting again on the new store
```

The line `store replaced` names what was lost. For a server that was only behind it says so instead, with the entry the old store ended at. The same line carries the path of the old store as `kept=`.

What to know about it:

- **The process starts again.** The server stops serving, runs its own binary again with the same arguments and environment, and opens the new store. The process id stays the same. On three local servers the gap was under a tenth of a second. If running the binary fails, for example because it was replaced on disk with one that does not start, the process exits with an error and `Restart=on-failure` in the shipped unit starts it. Without a service manager you then start it yourself, with the usual flags; `--join` and a token are not needed.
- **It keeps its place.** The server has the same id and the same role afterwards. A voter comes back as a voter and counts toward the majority as soon as it has pulled once. It is not demoted to a learner: it carries its own term and vote over to the new store, and the snapshot is the log of the leader it follows, which holds every confirmed write.
- **The old store is kept.** The two newest `state.redb.replaced-<time>` files stay in the data directory and older ones are deleted. The file is a whole store and holds the cluster's secrets, like `state.redb`. To look at the writes that were lost, copy it into an empty directory as `state.redb` and run `pistack --data-dir <that directory> store dump`. Nothing puts those writes back; apply them again by hand if they are wanted.
- **Not while leading.** A leader never replaces its store, and a follower does it only with a snapshot from the leader it heard from within the last election timeout. A snapshot from an older term than the server's own is refused.
- **Not more than once in ten minutes.** If the leader refuses the server again within ten minutes of a replacement, the server logs that it is waiting and leaves the store alone until the ten minutes are over. The time is read from the name of the newest `state.redb.replaced-<time>` file. A download or check that fails leaves the old store untouched and is tried again after 30 seconds, then one minute, doubling up to ten minutes.
- **Not a removed server.** A server taken out with `pi server remove` is refused the snapshot and does not come back this way. See [Removing and replacing a server](#removing-and-replacing-a-server).
- **Not after a restore.** A server still running on its old store when another was restored from a backup is not in the restored member list, so the snapshot is refused and it logs `the leader's member list does not name this server`. Empty it and join it again as the restore section says.

`PISTACK_AUTO_RESYNC=0` turns this off. The server then logs one of these, stops replicating, and stays in the member list as a voter that holds nothing new until you act:

```text
replication stopped: the leader no longer keeps the entries this store needs. Stop this node, move state.redb aside (keep node.id), and start it with --join <leader api>
```

```text
replication stopped: this store has entries the leader never had. Stop this node, remove state.redb (keep node.id), and start it with --join <leader api>
```

The procedure by hand:

```sh
# on the server that logged the message, after stopping pistack
mv /var/lib/pistack/state.redb /var/lib/pistack/state.redb.old
PISTACK_JOIN_TOKEN=pij_... pistack --data-dir /var/lib/pistack server --join http://10.0.0.12:8443 --listen 10.0.0.11:8443
```

Leave `node.id` in place so the server keeps its member identity. Done by hand it needs a join token, comes back as a learner and is promoted again when it has caught up.

## Removing and replacing a server

```sh
pi cluster status
pi server remove http://10.0.0.13:8443
```

`pi server remove` takes a server out of the member list. It names the server by id or by API address, as `pi cluster status` prints them, and needs the cloud admin. Only the leader makes the change. The change goes through the log like any other write and the answer waits for a majority of the voters that are left, so every server agrees on it. From then on the removed server does not count toward the majority.

The leader does not remove itself. Stop it, wait for another server to take over, and remove it from there.

A removal that would leave too few voters answering to form a majority is refused. A voter counts as answering when it pulled from the leader in the last three seconds. `--force` skips that check and nothing else. If the voters that are left really cannot form a majority, writes stop until they come back. Removing a learner is never refused for this reason.

To replace a server that is gone for good, remove it and then add a new one as in [Adding servers](#adding-servers). After one of three voters is removed there are two, and both must be up for a write to be confirmed. That lasts until the new server has caught up and been promoted, so do the two steps together.

The other servers refuse a removed server from then on, and it logs that it was removed. It does not come back by being restarted. To bring the same machine back, stop it, move `state.redb` aside and start it with `--join` and a join token, as for a new server. The token is needed even with `PISTACK_REQUIRE_JOIN=0`.

Two things removal does not do. It needs a leader: when the majority of voters is gone there is no leader, and the dead servers cannot be removed until enough of them return. And it does not take back what the removed server holds. Its `state.redb` still has the cluster's secrets, including the key members prove themselves with, and the others tell it apart only by the id it states. Destroy that file if the machine leaves your hands.

## Backup

```sh
pistack --data-dir /var/lib/pistack store backup --to /backup/pistack-2026-11-12.redb
```

This copies `state.redb` to the file, checks the copy, and prints the path. The destination must not exist. It can be run while the server is up.

The check opens the copy, verifies its checksums and reads every row. A copy that fails is deleted and the command fails. The store's schema version is printed on standard error and is recorded in the backup.

When the server is running, the store is locked and the command copies the file without holding a write transaction. The copy is checked as above, but for a backup you intend to rely on, take it from a follower you have stopped, or stop the server briefly.

The backup holds everything in the store, including secrets and the signing key for tokens. With the default `software` root-key backend it also holds the key that encrypts the secrets. Treat the file accordingly.

The store does not hold image files, local volume files or object data. Those are files under the data directory; see [Storage](Storage).

## Restore

```sh
# with no pistack server running on the data directory
pistack --data-dir /var/lib/pistack store restore --from /backup/pistack-2026-11-12.redb
pistack --data-dir /var/lib/pistack server --listen 10.0.0.11:8443
```

`store restore` copies the backup into the data directory, checks the copy the way `store backup` does, and only then puts it in place as `state.redb`. A backup that is cut short or damaged, that was written by a newer `pistack`, or that is not the store of a server that has run is refused, and the data directory is left as it was. The backup file itself is never changed.

The data directory must be empty or absent. With `--force` the command restores into a directory that has things in it: the `state.redb` and `admin-password` that are there are renamed to `state.redb.before-restore-<time>` and `admin-password.before-restore-<time>`, `adopt.url`, `state.partial` and `state.resync` are removed, and everything else is left alone. It refuses while a server has the store open. Delete the renamed files yourself once you no longer want them.

A restore founds the cluster again with one server. The member list in the backup is dropped, so the server you start on the restored store is the only member and the leader, whether it is the machine the backup came from or a new one. It keeps the cluster id, so tokens, API keys and the admin password are those of the backup. `--init` and `--join` are not needed.

What a restore does not put back:

- **Other servers.** Restore one server and start it. On every other server, stop `pistack`, empty the data directory or move `state.redb` aside, and start it with `--join` as a new server. Do this before they can reach the restored server: a server still running on its old store holds writes from after the backup and the same cluster key. The restored server raises the term so that such a server is told its log has diverged. It then asks for a snapshot and is refused, because the restored member list does not name it. This has not been tried on a real cluster.
- **Anything written after the backup.** Machines, volumes and addresses created since are unknown to the restored store, and ones deleted since are back in it. What is running on the hosts is not rolled back with it, and what the control plane then does about the difference has not been tested.
- **The root key in Vault.** With the default `software` backend the root key is in the backup. With the `vault` backend the backup holds the wrapped key and the server needs the same Vault settings and a Transit key version that can still unwrap it. `store restore` prints which of the two the backup holds; run `pistack keep status` to check before starting.
- **Files outside the store.** `tls/`, `agent/` and the module directories are not touched. A certificate in `tls/` is reused as it is. If the cloud CA was made after the backup was taken, remove `tls/` so the server issues a new one, and enroll the agents again.

## Upgrades

The store records a schema version. A server writes it when it first opens a store and checks it on every start. It is 1 today, and a store written before the version existed counts as 1.

A `pistack` that is older than the store refuses to open it, says which version the store is at, and changes nothing. A `pistack` that is newer runs its migration steps in order when it opens the store and records the new version. Every command that opens the store does this: `server`, `store stats`, `store dump`, `store restore` and `keep`. `store backup` copies the file and reports its version without migrating it. No release has needed a migration yet.

`pistack store stats`, `pi version` and the `/` document of a running server show the version.

Take a backup before replacing the binary. A migrated store cannot be opened by the old binary again; going back means restoring that backup with the old binary.

Each server migrates its own copy of the store. Servers do not compare schema versions with each other, so when a release raises the version, stop every server, replace the binary on all of them, and start them again. A binary from before the version was recorded does not check it at all.

## Looking at the store

```sh
pistack --data-dir /var/lib/pistack store stats
pistack --data-dir /var/lib/pistack store dump
```

`stats` prints rows and bytes per table and the range of the replication log. `dump` prints every resource as a line of JSON. Both open the store file, so the server must be stopped first.

## Files in the data directory

| Path | What it is |
| --- | --- |
| `state.redb` | The state store: all resources, cluster membership and the replication log. |
| `state.redb.before-restore-<time>` | The store that was here before `store restore --force`. |
| `state.redb.replaced-<time>` | The store this server had before it replaced it with a snapshot from the leader. The two newest are kept. |
| `state.resync` | A snapshot that was checked and is about to become `state.redb`. |
| `state.restore` | A backup being checked by `store restore`. |
| `admin-password` | The generated admin password, written when the cluster was founded. |
| `node.id` | This server's member id. Keep it across a rejoin. |
| `node.name` | The node name, chosen once. `PISTACK_NODE_NAME` overrides and rewrites it. |
| `adopt.url` | The join target, present only while a join is in progress. |
| `state.partial` | A snapshot download in progress, for a join or for a store replacement. |
| `tls/` | Certificate, key and CA for `--tls-listen`. |
| `agent/` | The agent's certificate, on a machine that also runs an agent. |
| `images/`, `volumes/`, `silo/`, `shares/` | Module data. See [Storage](Storage). |

## Settings that must match across servers

The network backend is cluster-wide. The leader saves `PISTACK_FABRIC_BACKEND` in the store and servers started without the variable use the saved value. Set it the same way on every server, or set it on the first and leave it unset on the rest.

The root-key backend (`PISTACK_KEEP_BACKEND` and the Vault settings) must be the same on every server. The `pistack keep` commands change one server's store and have to be run on each.
