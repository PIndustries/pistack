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

The password comes from `PISTACK_ADMIN_PASSWORD` if it is set. Otherwise the server generates one and writes it to the log once, at warning level. Change it, or at least take it out of the log.

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
```

Only an admin can create, list or delete projects.

Every token is scoped to one project, fixed when the token is issued. The project is chosen in this order:

1. the project named in the login request (`pi login --scope-project`),
2. the project of the API key, when logging in with one,
3. the principal's default project.

The scope decides which resources the token sees and where new resources go. There is no header or path segment to switch project on a single request. To work in another project, log in again with `--scope-project`, usually into a second profile:

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

Role changes take effect on the next token. A token already issued keeps the roles it was issued with until it expires.

### What is enforced today

Be careful with `reader`.

- Any token with the `admin` role is treated as a full admin, including when the binding is only in one project. A project admin can list hosts and create users.
- `gate` and `keep` enforce the difference between `member` and `reader`.
- The other modules check only that the token is scoped to the resource's project. A `reader` token can create and delete machines, networks, volumes and the rest in its project.

Pull request [PIndustries/pistack#103](https://github.com/PIndustries/pistack/pull/103) makes `reader` read-only everywhere. It is not merged yet. Until it is, give `reader` only to principals you would also trust with `member`.

`pi role create` makes a custom role. A custom role can be bound, and it grants nothing, because authorization only knows the three built-in roles. The policy engine in `crates/pistack-policy` is not wired in. `GET /identity/policy` describes the rules actually in force.

## Tokens

```sh
pi login http://10.0.0.11:8443 --name alice
```

`pi login` posts the name and password to `/identity/tokens` and stores the token in a profile. `--ttl` sets the lifetime in seconds.

- Tokens are signed JSON Web Tokens (HS256). The signing key is generated on first start and kept in the store.
- The default lifetime is 12 hours. The maximum is 7 days.
- A token carries the principal, project and roles. The server verifies it without looking anything up.

There is no way to revoke a token. Disabling or deleting a principal stops new logins and does not invalidate tokens already issued. Keep lifetimes short for that reason.

`pi` reads its connection from a profile file: `~/.config/pistack/config.toml` on Linux, `~/Library/Application Support/pistack/config.toml` on macOS, or the file named by `--config` or `PI_CONFIG`. `--url` and `--token` (or `PI_URL` and `PI_TOKEN`) override the profile for one command.

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

Pass the token to `pi` with `--token` or `PI_TOKEN`. `pi key delete` revokes the key. Tokens already exchanged from it stay valid until they expire.

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

Secret payloads and CA keys are encrypted with one root key. By default the root key is kept in the store itself, which means the store file alone is enough to read every secret. `PISTACK_KEEP_BACKEND=vault` keeps the root key wrapped by a HashiCorp Vault Transit key instead, so the store holds only ciphertext and the server asks Vault to unwrap it at start. The Vault settings are in the [Configuration reference](Configuration-reference). The source notes that the Vault backend has not been run against a production Vault.

Moving between the two is an offline operation on a stopped server, run on every control-plane node:

```sh
pistack keep status
pistack keep migrate-to-vault
pistack keep migrate-to-software --allow-plaintext-key
pistack keep rewrap
```

`rewrap` re-encrypts the root key under the newest version of the Transit key after you rotate it.

## Audit

Every request that changes something is recorded in the store with its method, path, principal and response status. Request bodies, query strings and headers are not recorded. There is no API to read the records yet; `pistack store dump` on a stopped server prints them with everything else.
