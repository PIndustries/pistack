// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Secrets and certificates (`keep`).
//!
//! A payload leaves here only through `secret get`, and arrives only from a
//! file or stdin: never as an argument, so it stays out of the shell history
//! and the process list.

use super::args::*;
use super::client::{CliError, CliResult};
use super::commands::{
    compact, confirm, create_body, do_create, do_delete, do_list, do_show, kind, opt, report,
    resolve,
};
use super::kinds::KindDef;
use super::output::{print_list, print_one, str_at, Output};
use super::{Ctx, Io};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::{json, Value};

const SECRET: &str = "secret";
const CERT: &str = "cert";

/// The CA certificate an issuer names is `ca-<uuid>`. `keep` looks the
/// issuer up by that id alone, so it is passed through untouched.
fn is_ca_id(s: &str) -> bool {
    uuid::Uuid::parse_str(s.strip_prefix("ca-").unwrap_or_default()).is_ok()
}

/// `--issuer` names a certificate authority. There is no `pi ca` to look one
/// up by name yet, so an id is required and the hint says where to find it.
fn issuer_id(issuer: Option<&str>) -> CliResult<Value> {
    match issuer.filter(|s| !s.is_empty()) {
        None => Ok(Value::Null),
        Some(s) if is_ca_id(s) => Ok(json!(s)),
        Some(s) => Err(CliError::usage(format!(
            "--issuer takes a certificate authority id (ca-…); '{s}' is not one. \
             `pi api GET /secrets/cas` lists them"
        ))),
    }
}

/// The wire spellings of the two enums the CLI takes on the command line.
fn kind_name(k: SecretKindArg) -> &'static str {
    match k {
        SecretKindArg::Opaque => "opaque",
        SecretKindArg::Password => "password",
        SecretKindArg::SshKey => "ssh_key",
        SecretKindArg::Token => "token",
        SecretKindArg::Certificate => "certificate",
    }
}

fn usage_name(u: CertUsageArg) -> &'static str {
    match u {
        CertUsageArg::Server => "server",
        CertUsageArg::Client => "client",
        CertUsageArg::Both => "both",
    }
}

/// `@path` and a bare path both read a file; `-` reads stdin. A payload is
/// never an argument: a command line is shell history and a process listing,
/// and neither is somewhere a secret belongs.
fn read_value(io: &mut Io<'_>, source: &str) -> CliResult<Vec<u8>> {
    if source == "-" {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut io.stdin, &mut buf)
            .map_err(|e| CliError::usage(format!("reading stdin: {e}")))?;
        return Ok(buf);
    }
    let path = source.strip_prefix('@').unwrap_or(source);
    std::fs::read(path).map_err(|e| CliError::usage(format!("reading {path}: {e}")))
}

/// `{"payload", "encoding"}` for a create or a put. Bytes go over as base64
/// so a binary payload survives the trip.
fn payload_body(io: &mut Io<'_>, value_file: &Option<String>) -> CliResult<Value> {
    let source = match value_file {
        Some(f) => f.as_str(),
        // A terminal has nothing to offer; say so rather than wait for a
        // payload nobody meant to type.
        None if io.tty => {
            return Err(CliError::usage(
                "no value given: pass --value-file, or pipe the value on stdin",
            ))
        }
        None => "-",
    };
    let mut bytes = read_value(io, source)?;
    // A trailing newline is how a pipe and an editor end a file, not part of
    // the secret.
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err(CliError::usage(
            "the value is empty; a secret has to hold something",
        ));
    }
    Ok(json!({
        "payload": B64.encode(&bytes),
        "encoding": "base64",
    }))
}

/// The bytes of a payload response, as text.
fn decode_payload(v: &Value) -> CliResult<String> {
    let raw = str_at(v, "/payload");
    if str_at(v, "/encoding") != "base64" {
        return Ok(raw);
    }
    let bytes = B64.decode(raw.trim()).map_err(|e| {
        CliError::usage(format!("the server sent a payload that is not base64: {e}"))
    })?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The payload the caller asked for: the bytes and nothing else, whatever
/// `-o` says, so a secret can be piped straight into a config file.
fn write_payload(io: &mut Io<'_>, v: &Value) -> CliResult<()> {
    let text = decode_payload(v)?;
    std::io::Write::write_all(io.stdout, text.as_bytes())?;
    if !text.ends_with('\n') {
        std::io::Write::write_all(io.stdout, b"\n")?;
    }
    Ok(())
}

/// `pi secret get NAME [--version N]`. keep keeps one payload per secret and
/// stamps the response with its version, so a version that is not the current
/// one is refused rather than printed. A version kept elsewhere — an agent's
/// cache — is re-read from its own record, never by guessing at an address.
async fn secret_get(ctx: &Ctx, io: &mut Io<'_>, name: &str, want: Option<u64>) -> CliResult<()> {
    let c = ctx.client()?;
    let id = resolve(&c, kind(SECRET), name).await?;
    let v = c
        .get_value(&format!("/secrets/secrets/{id}/payload"))
        .await?;
    let got = str_at(&v, "/version");
    if let Some(want) = want {
        if !got.is_empty() && got != want.to_string() {
            return Err(CliError::usage(format!(
                "secret \"{name}\" is at version {got}; version {want} is not kept"
            )));
        }
    }
    if matches!(ctx.fmt.output, Output::Table | Output::Wide) && !got.is_empty() {
        // Which version is coming out, on the channel nobody redirects.
        let last = str_at(&v, "/last_rotated");
        if last.is_empty() {
            writeln!(io.stderr, "version {got}")?;
        } else {
            writeln!(io.stderr, "version {got}, rotated {last}")?;
        }
    }
    write_payload(io, &v)
}

/// The next version of a secret that already exists.
///
/// The payload update is a conditional write. The revision of the record read
/// during the lookup goes out as `If-Match`, so a rotation that loses a race
/// fails instead of quietly overwriting what the winner wrote; a record that
/// reports no revision is put unconditionally.
async fn secret_put(ctx: &Ctx, io: &mut Io<'_>, a: SecretPut) -> CliResult<()> {
    let k = kind(SECRET);
    let body = payload_body(io, &a.value_file)?;
    let c = ctx.client()?;
    let cur = c.get_value(&k.item_path(&a.name)).await?;
    let id = match str_at(&cur, "/id") {
        s if s.is_empty() => resolve(&c, k, &a.name).await?,
        s => s,
    };
    let path = format!("/secrets/secrets/{id}/payload");
    let rev = str_at(&cur, "/revision");
    let v = if rev.is_empty() {
        c.put(&path, &body).await?
    } else {
        c.put_if_match(&path, &body, &rev).await?
    };
    report(ctx, io, k, &v, "put")
}

pub async fn secret(ctx: &Ctx, cmd: SecretCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind(SECRET);
    match cmd {
        SecretCmd::List(a) => {
            // The service answers a list from the token's project and has no
            // way to widen it, so `--all` asks once per project the caller can
            // work in and joins the answers. A project that cannot be read is
            // skipped: the rest are still worth listing.
            if !a.all {
                return do_list(ctx, io, k, a).await;
            }
            let c = ctx.client()?;
            let mut out: Vec<Value> = Vec::new();
            // The projects a token may switch into are exactly the ones worth
            // asking about; one request each, then one table.
            let mut scopes: Vec<String> = Vec::new();
            if let Ok(v) = c.get_value("/identity/tokens/self/projects").await {
                if let Some(items) = v.as_array() {
                    scopes = items
                        .iter()
                        .map(|p| str_at(p, "/id"))
                        .filter(|id| !id.is_empty())
                        .collect();
                }
            }
            if scopes.is_empty() {
                // Nothing to switch into: the token sees one scope anyway, and
                // an answer that is not a page — or an identity it cannot
                // read — is not this listing's loss. The rows it carries are
                // listed once however many requests could see them.
                if let Ok(page) = c.get_value(k.path).await {
                    if let Some(items) = page.get("items").and_then(Value::as_array) {
                        out.extend(items.iter().cloned());
                    }
                }
            }
            for id in scopes {
                if let Ok(page) = c.get_value(&format!("{}?project={id}", k.path)).await {
                    if let Some(items) = page.get("items").and_then(Value::as_array) {
                        out.extend(items.iter().cloned());
                    }
                }
            }
            out.sort_by_key(|v| str_at(v, "/name"));
            out.dedup_by(|a, b| str_at(a, "/id") == str_at(b, "/id"));
            print_list(io.stdout, ctx.fmt, k, &out)?;
            Ok(())
        }
        SecretCmd::Show(a) => do_show(ctx, io, k, a).await,
        SecretCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        SecretCmd::Get(a) => secret_get(ctx, io, &a.name, a.version).await,
        SecretCmd::Put(a) => secret_put(ctx, io, a).await,
        SecretCmd::Create(a) => {
            // keep's create body is flat: the payload sits beside `name`.
            let Value::Object(mut body) = create_body(
                k,
                &a.name,
                &a.meta,
                json!({
                    "kind": kind_name(a.kind),
                    "content_type": a.content_type,
                    "expires_at": opt(a.expires_at),
                    "rotate_after_days": a.rotate_after_days,
                }),
            )?
            else {
                unreachable!("a create body is always an object");
            };
            let Value::Object(payload) = payload_body(io, &a.value_file)? else {
                unreachable!("a payload is always an object");
            };
            body.extend(payload);
            let c = ctx.client()?;
            let v = c.post(k.path, &compact(Value::Object(body))).await?;
            report(ctx, io, k, &v, "created")
        }
    }
}

/// `POST /certificates/{id}/revoke`. Said as a revocation, not a deletion:
/// the record stays so its serial stays in the CRL.
async fn revoke(ctx: &Ctx, io: &mut Io<'_>, k: &KindDef, name: &str, yes: bool) -> CliResult<()> {
    let c = ctx.client()?;
    let id = resolve(&c, k, name).await?;
    if !yes && !confirm(io, "revoke", &format!("{} \"{}\"", k.noun, name))? {
        writeln!(io.stderr, "aborted")?;
        return Ok(());
    }
    let v = c
        .post(&format!("/secrets/certificates/{id}/revoke"), &json!({}))
        .await?;
    let shown = match str_at(&v, "/name") {
        s if s.is_empty() => name.to_owned(),
        s => s,
    };
    match ctx.fmt.output {
        Output::Table | Output::Wide => writeln!(io.stdout, "{} \"{shown}\" revoked", k.noun)?,
        Output::Name => writeln!(io.stdout, "{shown}")?,
        _ => print_one(io.stdout, ctx.fmt, &v)?,
    }
    Ok(())
}

pub async fn cert(ctx: &Ctx, cmd: CertCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind(CERT);
    match cmd {
        CertCmd::List(a) => do_list(ctx, io, k, a).await,
        CertCmd::Show(a) => do_show(ctx, io, k, a).await,
        CertCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        CertCmd::Revoke(a) => revoke(ctx, io, k, &a.name, a.yes).await,
        CertCmd::Issue(a) => {
            let spec = json!({
                "common_name": a.common_name.unwrap_or_else(|| a.name.clone()),
                "sans": a.sans,
                "usage": usage_name(a.usage),
                "ttl_days": a.ttl_days,
                "issuer": issuer_id(a.issuer.as_deref())?,
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
        CertCmd::Key(a) => {
            let c = ctx.client()?;
            let id = resolve(&c, k, &a.name).await?;
            let v = c
                .get_value(&format!("/secrets/certificates/{id}/key"))
                .await?;
            if !matches!(ctx.fmt.output, Output::Table | Output::Wide) {
                return Ok(print_one(io.stdout, ctx.fmt, &v)?);
            }
            for (field, header) in [
                ("cert_pem", "certificate"),
                ("key_pem", "private key"),
                ("ca_pem", "ca chain"),
            ] {
                let pem = str_at(&v, &format!("/{field}"));
                if pem.is_empty() {
                    continue;
                }
                writeln!(io.stdout, "----- {header} -----")?;
                std::io::Write::write_all(io.stdout, pem.as_bytes())?;
                if !pem.ends_with('\n') {
                    std::io::Write::write_all(io.stdout, b"\n")?;
                }
            }
            Ok(())
        }
    }
}

/// A secret in a manifest is the record without its payload, because a
/// manifest is a file and usually a committed one. The payload stays with
/// `secret put`. Nothing exists yet, so there is nothing to create: the
/// server would refuse an empty payload, and the caller is told what to run.
pub async fn apply(
    ctx: &Ctx,
    io: &mut Io<'_>,
    k: &KindDef,
    doc: &Value,
    existing: Option<Value>,
) -> CliResult<()> {
    let name = str_at(doc, "/name");
    let spec = doc.get("spec").cloned().unwrap_or_else(|| json!({}));
    let c = ctx.client()?;
    let Some(cur) = existing else {
        writeln!(
            io.stderr,
            "secret \"{name}\" not created: a manifest carries no payload. \
             Use `pi secret create {name} --value-file PATH`"
        )?;
        return Ok(());
    };
    let body = compact(json!({
        "description": doc.get("description").cloned().unwrap_or(Value::Null),
        "labels": doc.get("labels").cloned().unwrap_or(Value::Null),
        "content_type": spec.get("content_type").cloned().unwrap_or(Value::Null),
        "expires_at": spec.get("expires_at").cloned(),
        "rotate_after_days": spec.get("rotate_after_days").cloned(),
    }))
    .as_object()
    .expect("metadata is an object")
    .clone();
    if let Some(ct) = spec.get("kind") {
        // keep takes the kind only on create; a manifest that changes it is
        // a new secret, not an update.
        if str_at(&cur, "/spec/kind") != ct.as_str().unwrap_or_default() {
            writeln!(
                io.stderr,
                "secret \"{name}\" keeps kind {}; a kind is set when a secret is made",
                str_at(&cur, "/spec/kind")
            )?;
        }
    }
    // The revision read during the lookup goes out as `If-Match`: an apply
    // that lost the race to another writer fails instead of overwriting it.
    let path = k.item_path(&str_at(&cur, "/id"));
    let body = Value::Object(body);
    let rev = str_at(&cur, "/revision");
    if rev.is_empty() {
        c.put(&path, &body).await?;
    } else {
        c.put_if_match(&path, &body, &rev).await?;
    }
    writeln!(io.stdout, "secret \"{name}\" configured")?;
    Ok(())
}
