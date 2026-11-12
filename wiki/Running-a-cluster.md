# Running a cluster

A control plane is one or more `pistack server` processes that share a replicated state store. One server is already a cluster of one. Adding servers gives you copies of the state and automatic leader election. Hosts that run workloads are separate; see [Agents and hosts](Agents-and-hosts).

## Starting the first server

```sh
pistack --data-dir /var/lib/pistack server --init --listen 10.0.0.11:8443
```

`--init` founds a new cluster. The server creates `state.redb` in the data directory, becomes the only voter and the leader, and creates the `admin` project and principal. The admin password is taken from `PISTACK_ADMIN_PASSWORD` if set. Otherwise one is generated and written to the log once.

After the first start, `--init` is no longer needed. A server that finds an existing `state.redb` stays in that cluster whatever flags it is given.

### TLS

`--listen` always serves plain HTTP. If you do not pass `--insecure-http`, the server warns about it at start; the flag only acknowledges it and makes the default public URL `http://`.

`--tls-listen ADDR` adds an HTTPS listener next to the plain one. The address must be specific, not `0.0.0.0`. The certificate is issued by the cloud CA and kept in `<data_dir>/tls`. Clients do not have that CA in their trust store, so `pi` needs `--insecure` to use it today.

Until this is finished, put the API on a private network.

## Adding servers

```sh
pi join-token create server-2
PISTACK_JOIN_TOKEN=pij_... pistack --data-dir /var/lib/pistack server --join http://10.0.0.11:8443 --listen 10.0.0.12:8443
```

A new server needs a join token, in `PISTACK_JOIN_TOKEN`, to download the snapshot and to join. It needs it only for that first start: once it holds the store it is a member, and the token can expire. A server found by discovery needs the token too.

`--join URL` names any reachable address of the current leader. The new server downloads a snapshot of the store, registers as a member, and starts replicating. Only the leader serves joins; another server answers `409`.

With neither `--init` nor `--join`, a server with an empty data directory tries discovery, but only when `--listen` is a private IPv4 address. It probes every address in its /24 on UDP port 18445. If no cluster answers it founds one. If exactly one answers it joins it. If more than one answers it stops and asks for `--join`. With `--listen 0.0.0.0:8443` (the default) there is no discovery and the server exits, asking for `--init` or `--join`.

Cluster traffic between servers uses the same API port, under `/cluster`. Those routes hand out the whole store, so they answer only a member or a server holding a valid join token. Members prove themselves with a key derived from the token signing secret, which is in every member's store and in every snapshot; nothing has to be handed to them and it does not expire. A server that is refused says so and names `PISTACK_JOIN_TOKEN`.

`PISTACK_REQUIRE_JOIN=0` opens the routes to anything that reaches the port. Servers older than this check send no key, so replacing them one at a time stops replication between old and new; set `PISTACK_REQUIRE_JOIN=0` on every server for that one upgrade, then take it out.

`GET /cluster` on any server shows what it believes:

```sh
curl -s http://10.0.0.11:8443/cluster
```

```json
{"cluster_id":"...","node_id":"...","api":"http://10.0.0.11:8443",
 "leader_api":"http://10.0.0.11:8443","role":"Voter","leader":true,
 "members":[{"id":"...","api":"http://10.0.0.11:8443","role":"Voter"},
            {"id":"...","api":"http://10.0.0.12:8443","role":"Voter"},
            {"id":"...","api":"http://10.0.0.13:8443","role":"Voter"}],
 "term":1,"commit":74}
```

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

`pi` does not follow that address by itself. Point `pi` at the leader, or put a proxy in front of the servers that sends traffic to whichever one reports `"leader": true` on `/cluster`.

Agents do follow. An agent that loses its server tries the other servers it knows and registers with the new leader.

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

## When a server must rejoin

A follower that falls too far behind cannot catch up from the log. The leader keeps the last 10,000 entries, plus whatever a follower seen in the last ten minutes still needs, up to 200,000. A server that was down for longer logs one of these and stops replicating:

```text
replication stopped: the leader no longer keeps the entries this store needs. Stop this node, move state.redb aside (keep node.id), and start it with --join <leader api>
```

```text
replication stopped: this store has entries the leader never had. Stop this node, remove state.redb (keep node.id), and start it with --join <leader api>
```

The second message means this server accepted writes as leader that never reached the others. Those writes are lost when you remove the file. Keep a copy if you need to look at them.

The procedure is manual:

```sh
# on the server that logged the message, after stopping pistack
mv /var/lib/pistack/state.redb /var/lib/pistack/state.redb.old
pistack --data-dir /var/lib/pistack server --join http://10.0.0.12:8443 --listen 10.0.0.11:8443
```

Leave `node.id` in place so the server keeps its member identity. It comes back as a learner and is promoted again when it has caught up.

There is no command to remove a member from the cluster. A server that is gone for good stays in the member list.

## Backup

```sh
pistack --data-dir /var/lib/pistack store backup --to /backup/pistack-2026-11-12.redb
```

This copies `state.redb` to the file, opens the copy to check it, and prints the path. The destination must not exist. It can be run while the server is up.

When the server is running, the store is locked and the command copies the file without holding a write transaction. The copy is checked by opening it, but for a backup you intend to rely on, take it from a follower you have stopped, or stop the server briefly.

The backup holds everything in the store, including secrets and the signing key for tokens. With the default `software` root-key backend it also holds the key that encrypts the secrets. Treat the file accordingly.

There is no restore command. To restore, stop the server, put the backup in place as `<data_dir>/state.redb`, and start it. On a single server this brings back the state as of the backup, and tokens issued before it still work. If the file came from a different server, this server notices that the store was written by another node, forgets the vote stored in it and rejoins the leader as a learner. If there is no cluster left to rejoin, start it with the backup taken from that same server.

The store does not hold image files, local volume files or object data. Those are files under the data directory; see [Storage](Storage).

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
| `node.id` | This server's member id. Keep it across a rejoin. |
| `node.name` | The node name, chosen once. `PISTACK_NODE_NAME` overrides and rewrites it. |
| `adopt.url` | The join target, present only while a join is in progress. |
| `state.partial` | A snapshot download in progress. |
| `tls/` | Certificate, key and CA for `--tls-listen`. |
| `agent/` | The agent's certificate, on a machine that also runs an agent. |
| `images/`, `volumes/`, `silo/`, `shares/`, `gauge/` | Module data. See [Storage](Storage). |

## Settings that must match across servers

The network backend is cluster-wide. The leader saves `PISTACK_FABRIC_BACKEND` in the store and servers started without the variable use the saved value. Set it the same way on every server, or set it on the first and leave it unset on the rest.

The root-key backend (`PISTACK_KEEP_BACKEND` and the Vault settings) must be the same on every server. The `pistack keep` commands change one server's store and have to be run on each.
