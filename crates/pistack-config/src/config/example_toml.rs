// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A complete, commented example configuration that parses to the defaults.

/// A complete, commented example configuration. It parses to the defaults
/// (see the `example_parses` test), so it doubles as documentation of them.
pub const EXAMPLE_TOML: &str = r#"# pistack.example.toml
#
# Every key is optional; the values shown are the defaults. Any key may be
# overridden from the environment as PISTACK__<SECTION>__<KEY>, e.g.
# PISTACK__SERVER__LISTEN=0.0.0.0:8443 or PISTACK__DATA_DIR=/srv/pistack.

# Where the state store, TLS material and local image cache live.
data_dir = "/var/lib/pistack"

# Name this node reports as. Empty means "use the hostname".
node_name = ""

[server]
# Address the single API port binds to. Every module is a prefix on it.
listen = "0.0.0.0:8443"
# URL clients and agents use to reach this server. Empty means derive it
# from `listen` and the TLS setting.
public_url = ""
# Serve plain HTTP. Only for development; agents will not join over it.
insecure_http = false
# Seconds a long-poll (agent command stream, watch) may hang before the
# server answers with an empty result.
long_poll_secs = 30

[tls]
# PEM files for the API listener. When unset, `keep` issues a certificate
# from the cloud CA into <data_dir>/tls at first start.
# cert = "/etc/pistack/tls/server.crt"
# key = "/etc/pistack/tls/server.key"
# CA bundle used to verify agents and peer servers (mTLS). Defaults to the
# cloud CA in <data_dir>/tls/ca.crt.
# ca = "/etc/pistack/tls/ca.crt"

[log]
# tracing filter: error, warn, info, debug, trace, or a full EnvFilter.
level = "info"
# "text" or "json"
format = "text"

# Module-specific settings. One table per module, by workshop name.
# Each module reads its own with Config::module::<T>("name").
[modules]

# [modules.forge]
# scheduler = "spread"
# [modules.fabric]
# ovn_nb = "tcp:127.0.0.1:6641"
# [modules.slab]
# backends = ["lvm"]
# [modules.silo]
# # Largest single PUT in bytes (default 5 GiB).
# max_object_bytes = 5368709120
# # Seconds before an unfinished multipart upload is swept (default 7 days).
# multipart_ttl_secs = 604800
# [modules.keep]
# # Defaults for projects whose gate quotas do not name `secrets`,
# # `certificates` or `access_keys`; a project quota always wins.
# max_secrets_per_project = 1000
# max_certificates_per_project = 500
# max_access_keys_per_project = 100
# # Cloud-wide: unexpired, unspent join tokens at once, and the most
# # redemptions a multi-use token may allow (also its default).
# max_live_join_tokens = 100
# max_join_token_uses = 100
# # Expired or used-up join tokens are deleted by the leader this long
# # after they stop working (default 7 days).
# join_token_grace_secs = 604800
"#;
