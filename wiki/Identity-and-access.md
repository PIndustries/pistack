# Identity and access

The `gate` module owns principals, projects, roles, role bindings, tokens and API keys. The `keep` module owns access keys and join tokens. Identity is built in; there is no external identity provider to connect.

## How a request is authenticated

A request carries one of:

- `Authorization: Bearer <token>`, a token issued by `gate`.
- An AWS Signature Version 4 signature made with an access key. Only `silo` accepts this.

A request with no credentials is refused with `401` by the module it reaches. An invalid or expired token is refused with `401` before it reaches a module.

In `--dev` mode a request with no credentials is treated as an admin named `dev-admin` with no project. The server logs a warning at start when dev mode is on. Never run it where anyone else can reach the port.

## The first admin

The first time a server starts on an empty store it creates:

- the three built-in roles,
- a project named `admin`,
- a principal named `admin`, whose default project is `admin`,
- a system-wide binding of `admin` to the `admin` role.

The password comes from `PISTACK_ADMIN_PASSWORD` if it is set. Otherwise the server generates one and writes it to `<data_dir>/admin-password` with mode 0600; the log names the file and does not hold the password. Change the password and delete the file.

## Principals

A principal is a user or a service account. Names are unique across the cloud.

```sh
pi user create alice --email alice@example.org --password-stdin
pi user create ci --kind service-account
pi user list
```

Only an admin can create, list or delete principals. A principal can change its own password and email. Passwords are stored as argon2id hashes.

A service account is a principal for a program. It is meant to authenticate with an API key instead of a password.

## Projects and scope

```sh
pi project create research --quota machines=20
pi quota set research vcpus=64 volumes_gib=2000
pi quota show research
```

Only an admin can create, list or delete projects, or set their quotas. A member or reader of a project can read its limits and usage. The resources that can be limited are listed under [Quotas](Concepts#quotas).

Every token is scoped to one project, fixed when the token is issued. The project is chosen in this order:

1. the project named in the login request (`pi login --scope-project`),
2. the project of the API key, when logging in with one,
3. the principal's default project.

A token is only issued for a project the principal has a role in, or to a principal with a role at system scope. Asking for any other project is refused with `403`: a token with no role can still read, so one for a project you are not in would show you what it holds. A default project the principal has no role in is skipped, and the token has no project. A token for a project stops working when the principal's last role in it is removed.

The scope decides which resources the token sees and where new resources go. There is no header or path segment to switch project on a single request.

A caller can ask which projects they may work in and trade their token for one in another of them, without the password:

```sh
pi api GET /identity/tokens/self/projects
pi api POST /identity/tokens/self/switch -d '{"project": "research"}'
```

The first lists each project with the project it sits under and the roles the caller has there. Any caller may ask; listing every project is still for a cloud admin. The second returns a new token. It ends when the old one would have, so switching does not keep a session alive, and the old token goes on working until then. A token that came from an API key cannot be moved: the key names its project. The web console uses these two calls for the project box in its header.

With `pi`, log in again with `--scope-project`, usually into a second profile:

```sh
pi --profile research login http://10.0.0.11:8443 --name alice --scope-project research
pi --profile research machine list
```

The global `--project` option of `pi` supplies the default for `pi login`. It does not re-scope a token you already have.

## Roles and bindings

| Role | Meaning |
| --- | --- |
| `admin` | Full control over the cloud or a project. |
| `member` | Create, change and delete resources within a project. |
| `reader` | Read-only access within a project. |

A binding gives one principal one role, in one project or system-wide.

```sh
pi binding create --principal alice --role member --in-project research
pi binding create --principal alice --role admin
pi binding list
```

Leaving out `--in-project` makes the binding system-wide.

When a token is issued, the server works out its roles:

- System-wide bindings always apply.
- Bindings in a project apply only when the token is scoped to that project.
- A system-wide `admin` binding marks the token as a system token.

`pi whoami` shows the result:

```text
principal  alice
id         prin-01a1ce37-8c29-727f-8fef-aed919a61b47
project    proj-01a1ce37-8b04-71c8-85ae-7622e1d435df
roles      member
system     false
```

Role changes take effect on the next request. The server reads the bindings each time and does not trust the roles written in the token.

### What is enforced today

Be careful with `reader`.

- `admin` bound system-wide is the cloud admin. `admin` bound in one project writes there like a `member`. In `keep` it can also read a certificate's key and create an access key for another principal of that project. It cannot see other projects, manage users, publish public images or create join tokens.
- `gate` and `keep` enforce the difference between `member` and `reader`.
- The other modules check only that the token is scoped to the resource's project. A `reader` token can create and delete machines, networks, volumes and the rest in its project.

Pull request [PIndustries/pistack#103](https://github.com/PIndustries/pistack/pull/103) makes `reader` read-only everywhere. It is not merged yet. Until it is, give `reader` only to principals you would also trust with `member`.

`pi role create` makes a custom role. A custom role can be bound, and it grants nothing, because authorization only knows the three built-in roles. The policy engine in `crates/pistack-policy` is not wired in. `GET /identity/policy` describes the rules actually in force.

## Tokens

```sh
pi login http://10.0.0.11:8443 --name alice
```

`pi login` posts the name and password to `/identity/tokens` and stores the token in a profile. `--ttl` sets the lifetime in seconds.

- Tokens are signed JSON Web Tokens (HS256). The signing keys are kept in the store. The newest one signs, and a token names the key it was signed with in its `kid` header.
- The default lifetime is 12 hours. The maximum is 7 days.
- A token carries the principal and project. On every request the server checks that the principal still exists and is enabled, and reads its roles.

Disabling or deleting a principal refuses its tokens from the next request, with `401 token revoked`.

A single token cannot be cancelled. The server keeps no list of the tokens it has issued, so there is nothing to mark. What you can do is refuse every token one user holds:

```sh
pi user revoke-tokens alice
```

This stamps the user with the current time, and any token of theirs issued at or before it gets `401 token revoked` from the next request on, on every server. The user logs in again for a new one. A login in the same second as the stamp is still refused; one second later it works. An admin can do this for anyone and a user can do it for themselves. API keys are not touched: delete the key to stop logins with it.

`pi` reads its connection from a profile file: `~/.config/pistack/config.toml` on Linux, `~/Library/Application Support/pistack/config.toml` on macOS, or the file named by `--config` or `PI_CONFIG`. `--url` and `--token` (or `PI_URL` and `PI_TOKEN`) override the profile for one command.

### Rotating the signing key

```sh
pi signing-key list
pi signing-key rotate
pi signing-key retire
```

`rotate` makes a new key and signs with it from then on. The key it replaces keeps verifying for 7 days and 5 seconds, which is the longest a token can live plus the clock skew allowed, and then stops by itself. Nobody is logged out and there is nothing more to run. `list` shows each key's id, its age and the time it stops verifying.

`retire` drops every key but the current one at once. Every token signed with a dropped key fails with `401 invalid token` from the next request, so everyone logs in again. Use it after `rotate` when the old key may have leaked. It asks for confirmation; `--yes` skips the question.

Nothing rotates the key on a schedule. Each rotation is an admin's command, and it is written to the audit log like any other change. A rotation made on the leader reaches the other servers with the store, and each server uses the new set from its next request. No restart is needed.

Only the key ids and dates are ever returned. The key material stays in the store, so a copy of `state.redb` is still enough to sign tokens for that cloud.

### Upgrading from a build without key ids

A cloud started before signing keys had ids holds one secret. After the upgrade that secret is the first key of the set, under an id computed from it, so every server names it the same way. Tokens issued before the upgrade carry no `kid` and are checked against the oldest key, so they keep working.

During a rolling upgrade both builds sign with the same secret. The old build ignores `kid`, so each accepts the other's tokens.

Upgrade every server before the first `pi signing-key rotate`:

- After a rotation, a server still on the old build refuses tokens signed with the new key (`401 invalid token`), because it only knows the old secret. It still accepts the old ones.
- When the first key is retired, by `retire` or by a later `rotate` after its 7 days, the old secret is deleted from the store. A server on the old build then cannot recognise the other servers or be recognised by them, and if it is restarted it writes a new secret into its own copy of the store. Do not run the old build after that.

## API keys

An API key is a long-lived secret for a principal, meant for scripts and service accounts.

```sh
pi key create deploy --principal ci --scope-project research
```

```text
key "ci/deploy" created (key-ce1b3d83)
secret (shown once): pik_...
```

The secret starts with `pik_`. The server stores only a hash, so it cannot be shown again. `--expires-at` takes an RFC 3339 time; without it the key does not expire. A principal can create keys for itself. Creating one for someone else needs admin.

A key is not sent on API requests. It is exchanged for a token:

```sh
curl -s -X POST http://10.0.0.11:8443/identity/tokens \
  -H 'content-type: application/json' \
  -d '{"api_key":"pik_..."}'
```

```json
{"token":"...","token_type":"Bearer","expires_at":"2026-11-12T17:25:09Z",
 "caller":{"principal_name":"ci","project":"proj-...","roles":["member"],"system":false}}
```

Pass the token to `pi` with `--token` or `PI_TOKEN`. `pi key delete` revokes the key and every token exchanged from it.

## Access keys

An access key is an id and a secret used to sign S3 requests to `silo`.

```sh
pi access-key create --description "backup job"
```

```text
access-key "PIAKBL6ZATPFBVHTTSQ6" created (sec-18e9e731)
secret_access_key (shown once): ...
```

The id starts with `PIAK`. The secret is shown once.

An access key belongs to a principal and a project. A request signed with it acts as that principal in that project, with the roles the principal has there. It never acts with system-wide rights, even when the principal is a cloud admin.

Creating one needs `member` or `admin`, and the principal must have a role bound in the token's project. This catches out the bootstrap admin, who has only a system-wide binding:

```text
error: principal prin-... has no role in project proj-...
```

Bind the admin in the project first:

```sh
pi binding create --principal admin --role admin --in-project admin
```

`pi access-key delete` revokes the key. Ids are never reused.

## Join tokens

A join token lets a host agent enroll. It starts with `pij_`, is created by an admin with `pi join-token create`, and is covered on [Agents and hosts](Agents-and-hosts).

## Secrets and the cloud CA

`keep` also stores secrets and issues certificates from the cloud CA (a root, `pistack-root`, and an issuing CA, `pistack-issuing`). There are no `pi` commands for these yet; the API is under `/secrets`.

### How long the cloud CA lasts

Both CAs are made the first time a server starts on a new store, with ECDSA P-256 keys:

| CA | Signed by | Valid for |
| --- | --- | --- |
| `pistack-root` | itself | 3650 days (10 years) |
| `pistack-issuing` | `pistack-root` | 1826 days (5 years) |

`pi api GET /secrets/cas` shows the exact dates in `status.not_after`.

Every other certificate is signed by `pistack-issuing` and never outlives it. A certificate asked for 365 days when the issuing CA has 200 days left is cut to those 200 days. So in the fifth year the agent and server certificates get shorter with every renewal, and on the issuing CA's last day they all expire together. The server does not warn about the CA's own date.

Neither CA can be rotated. There is no command for it and no procedure that works with the commands that exist:

- The server finds the issuing CA by its name and makes one only when no CA called `pistack-issuing` exists. It cannot hold an old and a new issuing CA side by side, and nothing deletes the old one.
- A new issuing CA signed by the same root would be trusted by the agents, because their `ca.pem` holds the root too. Every server would still need its `tls/` directory removed and a restart to get a certificate from it, and every agent certificate would be reissued at its next renewal or by enrolling again.
- A new root is trusted by nobody. Every server's `tls/` and every agent's `<data_dir>/agent/` would have to be removed, and every agent enrolled again with a join token.

Until rotation is built, the date of `pistack-issuing` is the date by which a build that can rotate it has to be running. Note it when you found a cloud.

Agent and server certificates are renewed without anyone's help well before that. See [Agents and hosts](Agents-and-hosts#certificate-renewal) and [Running a cluster](Running-a-cluster#tls).

### Where the root key is kept

Secret payloads and CA keys are encrypted with one root key. By default the root key is kept in the store itself, which means the store file alone is enough to read every secret. `PISTACK_KEEP_BACKEND=vault` keeps the root key wrapped by a HashiCorp Vault Transit key instead, so the store holds only ciphertext and the server asks Vault to unwrap it at start. The Vault settings are in the [Configuration reference](Configuration-reference). The source notes that the Vault backend has not been run against a production Vault.

Moving between the two is an offline operation on a stopped server, run on every control-plane node:

```sh
pistack keep status
pistack keep migrate-to-vault
pistack keep migrate-to-software --allow-plaintext-key
pistack keep rewrap
```

`rewrap` re-encrypts the root key under the newest version of the Transit key after you rotate it. It changes how the root key is wrapped, not the root key, and no secret is touched.

### Rotating the root key

With the `software` backend the root key itself can be replaced. Every secret payload and private key in the store is decrypted with the old key and encrypted again with a new one:

```sh
# on a single server, stopped
pistack keep rotate-root-key
```

A cluster keeps one copy of the store per server and the command changes only the copy it runs on, so every server must be given the same new key. Stop all the servers first:

```sh
# on the first server: makes a new key and writes it to the file
pistack keep rotate-root-key --key-file /root/new-root.key
# copy the file to each other server, then on each of them:
pistack keep rotate-root-key --key-file /root/new-root.key
# when every server is done, delete the file everywhere and start the servers
```

The command refuses to run without `--key-file` on a server that has other members.

It is safe to interrupt. The new key is saved next to the old one before any secret is touched, each secret is rewritten on its own, and the old key is only dropped after every payload has been opened once with the new one. Run the same command again to carry on from where it stopped. While a rotation is unfinished the server refuses to start on that store and says so, and `pistack keep status` shows it.

What it does not do:

- It never runs by itself. Nothing rotates the root key on a schedule.
- It does not work with `PISTACK_KEEP_BACKEND=vault`. There the root key is wrapped by Transit; rotate the Transit key and run `rewrap`. Replacing the root key itself under Vault is not built.
- It does not reach backups. A backup taken before the rotation holds the old key and everything sealed with it.

### Secrets that ask to be rotated

A secret can carry `rotate_after_days`. The server never rotates a secret. Once the payload is older than that many days, counted from the last time it was put or from creation, the leader logs a warning naming the secret, and repeats it every six hours until a new payload is put with `PUT /secrets/secrets/{id}/payload`.

The root key has the same kind of reminder. Set `root_key_rotate_after_days` under `[modules.keep]` and the leader warns once the key is older than that, with the command to run. The age counts from the last `rotate-root-key`, or from the day the cloud was founded for a key never rotated. With the Vault backend it counts from the last `rewrap`. It is off by default and it only warns.

## Audit

The server keeps an audit log in the store. It replicates with everything else and survives a restart.

### What is recorded

One record is written after the response for:

- every request that changes something: `POST`, `PUT`, `PATCH` and `DELETE` on any module, a login (`POST /identity/tokens`), and removing a control-plane server;
- three reads that hand out a secret or control of a resource: a secret's payload, a certificate's private key and a machine console ticket;
- an agent enrolling with a join token.

A request that was refused is recorded like one that worked, with the status it got. That includes a failed login, a write by a `reader`, and a request with a bad body.

Each record holds:

| Field | Meaning |
| --- | --- |
| `created_at` | When the response was sent. |
| `principal`, `principal_id` | Who made the request. For a failed password login, the name that was tried and no id. |
| `api_key` | The API key behind the token, when the token came from a key. |
| `project` | The project the caller's token is scoped to. |
| `client` | The address the connection came from, as the server saw it. Behind a proxy or load balancer this is the proxy; forwarding headers are not read. |
| `method`, `path` | The request line, without the query string. |
| `action` | What it did: `create`, `update`, `delete`, `login`, `enroll`, the name of an action route such as `drain`, `evacuate`, `start` or `revoke`, or a verb and the part it touched, such as `update_quotas`, `update_labels` or `read_payload`. |
| `kind`, `resource` | The collection in the path (`machines`, `bindings`, `hosts`) and the resource as the request named it, by id or by name. For a create, the id of what was made. |
| `status` | The HTTP status of the answer. |
| `request_id` | The `x-request-id` of the request, the same one the server log shows. |

Request bodies, query strings and headers are never stored, so a record cannot hold a password, a token or a secret value.

What is not recorded: ordinary reads; agent traffic under `/rpc` other than enrolment; object uploads and deletes in a bucket (creating and deleting the bucket is recorded); and what servers say to each other under `/cluster`, which includes a server joining.

### Reading it

Only a cloud admin can read the log. A project member cannot read the records of their own project: a record carries the project of the token that made the request, not of what the request touched, so a view by project would leave out what an admin did to that project.

```sh
pi audit list
pi audit list --since 2h --outcome failure
pi audit list --action login --principal alice
pi audit list --kind hosts --resource rack1 -o json
```

`pi audit list` prints the newest hundred records. `--since` and `--until` take an RFC 3339 time or an age such as `30m`, `2h` or `7d`. `--outcome` is `success` (a status below 400), `failure`, or one status code. When there are more records than `--limit`, `pi` prints the `--after` value that continues the list. `-o wide` adds the project, the API key, the method, the path and the record id.

The API is `GET /identity/audit` with the query parameters `since`, `until`, `principal`, `project`, `action`, `kind`, `resource`, `outcome`, `order` (`asc`, the default, or `desc`), `limit` (100 by default, at most 1000) and `after`. It answers like every other list: `items`, `total`, and `next` when there is another page.

A secret read is answered by the leader only. A follower cannot write the record, so it refuses the read with `409` and the leader's address, as it does for a write; `pi` follows that.

### Retention

The log is bounded by age and by count. Every five minutes the leader deletes records older than the age limit and, if there are still more than the count limit, the oldest of them.

| Variable | Default | Meaning |
| --- | --- | --- |
| `PISTACK_AUDIT_MAX_AGE_DAYS` | `90` | Days a record is kept. `0` turns the age limit off. |
| `PISTACK_AUDIT_MAX_RECORDS` | `50000` | Most records kept. It cannot be turned off. |

Set them the same on every server, since whichever leads does the deleting. A busy cloud reaches the count before the age: 50,000 records is about 35 days at one change a minute. Every query reads the whole log, so a much larger count makes `pi audit list` slower and the store bigger. To keep records longer than the limits allow, export them on a schedule with `pi audit list --since 1d --oldest-first -o json`.
