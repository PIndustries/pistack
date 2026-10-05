// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! One handler per command. Resources are handled as `serde_json::Value`
//! so the CLI never has to track every module's exact struct; the kind
//! table in `kinds.rs` says where things live and how they print.

use super::args::*;
use super::client::{CliError, CliResult, Client};
use super::config::Profile;
use super::kinds::{self, KindDef, UpdateStyle};
use super::output::{self, print_list, print_one, print_value, short_id, str_at, Output};
use super::{Ctx, Io};
use pistack_types::Page;
use reqwest::Method;
use serde_json::{json, Map, Value};
use std::time::Duration;

const POLL: Duration = Duration::from_secs(1);
const WAIT_TIMEOUT: Duration = Duration::from_secs(600);

pub async fn dispatch(ctx: &mut Ctx, cmd: Cmd, io: &mut Io<'_>) -> CliResult<()> {
    match cmd {
        Cmd::Login(a) => login(ctx, a, io).await,
        Cmd::Profile { cmd } => profile(ctx, cmd, io),
        Cmd::Whoami => whoami(ctx, io).await,
        Cmd::Modules => modules(ctx, io).await,
        Cmd::Version => version(ctx, io).await,
        Cmd::Api(a) => api(ctx, a, io).await,
        Cmd::Apply(a) => apply(ctx, a, io).await,
        Cmd::Completion { .. } => Ok(()),

        Cmd::Project { cmd } => project(ctx, cmd, io).await,
        Cmd::User { cmd } => user(ctx, cmd, io).await,
        Cmd::Role { cmd } => role(ctx, cmd, io).await,
        Cmd::Binding { cmd } => binding(ctx, cmd, io).await,
        Cmd::Key { cmd } => key(ctx, cmd, io).await,
        Cmd::Machine { cmd } => machine(ctx, cmd, io).await,
        Cmd::Size { cmd } => size(ctx, cmd, io).await,
        Cmd::Host { cmd } => host(ctx, cmd, io).await,
        Cmd::Network { cmd } => network(ctx, cmd, io).await,
        Cmd::Subnet { cmd } => subnet(ctx, cmd, io).await,
        Cmd::Port { cmd } => port(ctx, cmd, io).await,
        Cmd::Router { cmd } => router(ctx, cmd, io).await,
        Cmd::Secgroup { cmd } => secgroup(ctx, cmd, io).await,
        Cmd::Fip { cmd } => fip(ctx, cmd, io).await,
        Cmd::Volume { cmd } => volume(ctx, cmd, io).await,
        Cmd::Snapshot { cmd } => snapshot(ctx, cmd, io).await,
        Cmd::Voltype { cmd } => voltype(ctx, cmd, io).await,
        Cmd::Image { cmd } => image(ctx, cmd, io).await,
        Cmd::Statefulset { cmd } => statefulset(ctx, cmd, io).await,
        Cmd::Daemonset { cmd } => daemonset(ctx, cmd, io).await,
        Cmd::Autoscaler { cmd } => autoscaler(ctx, cmd, io).await,
        Cmd::Stack { cmd } => stack(ctx, cmd, io).await,
    }
}

// ---- small helpers ------------------------------------------------------------

fn kind(noun: &str) -> &'static KindDef {
    // Every noun this file names is in the table; a miss is a programming error.
    kinds::by_noun(noun).unwrap_or(&kinds::KINDS[0])
}

/// `k=v` pairs into a JSON object of strings.
fn parse_kv(pairs: &[String], what: &str) -> CliResult<Map<String, Value>> {
    let mut m = Map::new();
    for p in pairs {
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| CliError::usage(format!("{what} must be key=value, got '{p}'")))?;
        m.insert(k.trim().to_owned(), Value::String(v.trim().to_owned()));
    }
    Ok(m)
}

/// `k=N` pairs into a JSON object of integers.
fn parse_kv_u64(pairs: &[String], what: &str) -> CliResult<Map<String, Value>> {
    let mut m = Map::new();
    for p in pairs {
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| CliError::usage(format!("{what} must be key=N, got '{p}'")))?;
        let n: u64 = v
            .trim()
            .parse()
            .map_err(|_| CliError::usage(format!("{what}: '{v}' is not a number")))?;
        m.insert(k.trim().to_owned(), json!(n));
    }
    Ok(m)
}

/// `@file` reads the file; anything else is literal.
fn at_file(s: &str) -> CliResult<String> {
    match s.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| CliError::usage(format!("reading {path}: {e}"))),
        None => Ok(s.to_owned()),
    }
}

fn opt(v: Option<String>) -> Value {
    v.map(Value::String).unwrap_or(Value::Null)
}

/// Drop nulls so the server's `#[serde(default)]` applies.
fn compact(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, compact(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(compact).collect()),
        other => other,
    }
}

/// Name or id -> id, via `GET /<path>/{name}` when needed.
async fn resolve(c: &Client, k: &KindDef, name_or_id: &str) -> CliResult<String> {
    if k.is_id(name_or_id) {
        return Ok(name_or_id.to_owned());
    }
    let v = c.get_value(&k.item_path(name_or_id)).await.map_err(|e| {
        if e.is_not_found() {
            CliError::usage(format!("{} '{}' not found", k.noun, name_or_id))
        } else {
            e
        }
    })?;
    Ok(str_at(&v, "/id"))
}

async fn resolve_opt(c: &Client, k: &KindDef, v: &Option<String>) -> CliResult<Value> {
    match v {
        Some(s) => Ok(Value::String(resolve(c, k, s).await?)),
        None => Ok(Value::Null),
    }
}

async fn resolve_all(c: &Client, k: &KindDef, names: &[String]) -> CliResult<Value> {
    let mut out = Vec::with_capacity(names.len());
    for n in names {
        out.push(Value::String(resolve(c, k, n).await?));
    }
    Ok(Value::Array(out))
}

/// Create body in the shape the module expects.
fn create_body(k: &KindDef, name: &str, meta: &MetaArgs, spec: Value) -> CliResult<Value> {
    let labels = parse_kv(&meta.labels, "--label")?;
    let mut body = Map::new();
    body.insert("name".into(), json!(name));
    if let Some(d) = &meta.description {
        body.insert("description".into(), json!(d));
    }
    if !labels.is_empty() {
        body.insert("labels".into(), Value::Object(labels));
    }
    if k.flat_create {
        if let Value::Object(m) = compact(spec) {
            body.extend(m);
        }
    } else {
        body.insert("spec".into(), compact(spec));
    }
    Ok(Value::Object(body))
}

fn confirm(io: &mut Io<'_>, what: &str) -> CliResult<bool> {
    if !io.tty {
        return Err(CliError::usage(format!(
            "refusing to delete {what} without --yes (no terminal to confirm on)"
        )));
    }
    write!(io.stderr, "delete {what}? [y/N] ")?;
    io.stderr.flush()?;
    let line = io.read_line()?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

/// Say what happened in table mode; dump the resource otherwise.
fn report(ctx: &Ctx, io: &mut Io<'_>, k: &KindDef, v: &Value, verb: &str) -> CliResult<()> {
    match ctx.fmt.output {
        Output::Table | Output::Wide => {
            let id = str_at(v, "/id");
            if id.is_empty() {
                writeln!(io.stdout, "{} \"{}\" {verb}", k.noun, str_at(v, "/name"))?;
            } else {
                writeln!(
                    io.stdout,
                    "{} \"{}\" {verb} ({})",
                    k.noun,
                    str_at(v, "/name"),
                    short_id(&id)
                )?;
            }
        }
        Output::Name => writeln!(io.stdout, "{}", str_at(v, "/name"))?,
        _ => print_one(io.stdout, ctx.fmt, v)?,
    }
    Ok(())
}

// ---- generic verbs --------------------------------------------------------------

async fn list_all(c: &Client, k: &KindDef, a: &ListArgs) -> CliResult<Vec<Value>> {
    let mut query: Vec<String> = Vec::new();
    if a.all {
        query.push("all=true".into());
    }
    if !a.selector.is_empty() {
        for s in &a.selector {
            if !s.contains('=') {
                return Err(CliError::usage(format!(
                    "selector must be key=value, got '{s}'"
                )));
            }
        }
        query.push(format!("labels={}", a.selector.join(",")));
    }
    let mut items = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut q = query.clone();
        if let Some(a) = &after {
            q.push(format!("after={a}"));
        }
        let path = if q.is_empty() {
            k.path.to_owned()
        } else {
            format!("{}?{}", k.path, q.join("&"))
        };
        let page: Page<Value> = c.get(&path).await?;
        let got = page.items.len();
        items.extend(page.items);
        match page.next {
            Some(n) if got > 0 && Some(&n) != after.as_ref() => after = Some(n),
            _ => break,
        }
    }
    Ok(items)
}

async fn do_list(ctx: &Ctx, io: &mut Io<'_>, k: &KindDef, a: ListArgs) -> CliResult<()> {
    let c = ctx.client()?;
    let items = list_all(&c, k, &a).await?;
    print_list(io.stdout, ctx.fmt, k, &items)?;
    Ok(())
}

async fn do_show(ctx: &Ctx, io: &mut Io<'_>, k: &KindDef, a: NameArg) -> CliResult<()> {
    let c = ctx.client()?;
    let v = c.get_value(&k.item_path(&a.name)).await?;
    print_one(io.stdout, ctx.fmt, &v)?;
    Ok(())
}

async fn do_create(
    ctx: &Ctx,
    io: &mut Io<'_>,
    k: &KindDef,
    body: Value,
    wait: bool,
) -> CliResult<Value> {
    let c = ctx.client()?;
    let v = c.post(k.path, &body).await?;
    report(ctx, io, k, &v, "created")?;
    if wait {
        return wait_ready(&c, io, k, &str_at(&v, "/id")).await;
    }
    Ok(v)
}

async fn do_delete(ctx: &Ctx, io: &mut Io<'_>, k: &KindDef, a: DeleteArgs) -> CliResult<()> {
    let c = ctx.client()?;
    let path = k.item_path(&a.name);
    if !a.yes && !confirm(io, &format!("{} \"{}\"", k.noun, a.name))? {
        writeln!(io.stderr, "aborted")?;
        return Ok(());
    }
    let (status, _) = c.delete(&path).await?;
    let verb = if status == reqwest::StatusCode::ACCEPTED {
        "deletion requested"
    } else {
        "deleted"
    };
    writeln!(io.stdout, "{} \"{}\" {verb}", k.noun, a.name)?;
    if a.wait && status == reqwest::StatusCode::ACCEPTED {
        wait_gone(&c, io, k, &a.name).await?;
    }
    Ok(())
}

/// POST an action endpoint and report.
async fn do_action(
    ctx: &Ctx,
    io: &mut Io<'_>,
    k: &KindDef,
    name: &str,
    action: &str,
    body: Option<Value>,
) -> CliResult<Value> {
    let c = ctx.client()?;
    let path = format!("{}/{}", k.item_path(name), action);
    let v = match body {
        Some(b) => c.post(&path, &b).await?,
        None => c.post_empty(&path).await?,
    };
    report(ctx, io, k, &v, &format!("{action} requested"))?;
    Ok(v)
}

/// Poll until `status.phase` is `ready` or `error`, printing each change.
async fn wait_ready(c: &Client, io: &mut Io<'_>, k: &KindDef, id: &str) -> CliResult<Value> {
    let start = tokio::time::Instant::now();
    let mut last = String::new();
    loop {
        let v = c.get_value(&k.item_path(id)).await?;
        let phase = str_at(&v, "/status/phase");
        if phase != last {
            let msg = str_at(&v, "/status/message");
            if msg.is_empty() {
                writeln!(io.stdout, "{} \"{}\": {phase}", k.noun, str_at(&v, "/name"))?;
            } else {
                writeln!(
                    io.stdout,
                    "{} \"{}\": {phase} ({msg})",
                    k.noun,
                    str_at(&v, "/name")
                )?;
            }
            last = phase.clone();
        }
        match phase.as_str() {
            "ready" => return Ok(v),
            "error" => {
                return Err(CliError::usage(format!(
                    "{} \"{}\" failed: {}",
                    k.noun,
                    str_at(&v, "/name"),
                    str_at(&v, "/status/message")
                )))
            }
            _ => {}
        }
        if start.elapsed() > WAIT_TIMEOUT {
            return Err(CliError::usage(format!(
                "timed out waiting for {} \"{}\" (last phase: {phase})",
                k.noun,
                str_at(&v, "/name")
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn wait_gone(c: &Client, io: &mut Io<'_>, k: &KindDef, name: &str) -> CliResult<()> {
    let start = tokio::time::Instant::now();
    let mut last = String::new();
    loop {
        match c.get_value(&k.item_path(name)).await {
            Ok(v) => {
                let phase = str_at(&v, "/status/phase");
                if phase != last {
                    writeln!(io.stdout, "{} \"{name}\": {phase}", k.noun)?;
                    last = phase;
                }
            }
            Err(e) if e.is_not_found() => {
                writeln!(io.stdout, "{} \"{name}\": gone", k.noun)?;
                return Ok(());
            }
            Err(e) => return Err(e),
        }
        if start.elapsed() > WAIT_TIMEOUT {
            return Err(CliError::usage(format!(
                "timed out waiting for {} \"{name}\" to go away",
                k.noun
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

// ---- session commands -------------------------------------------------------------

async fn login(ctx: &mut Ctx, a: LoginArgs, io: &mut Io<'_>) -> CliResult<()> {
    let password = if a.password_stdin {
        io.read_line()?
    } else if io.tty {
        rpassword::prompt_password(format!("password for {}: ", a.name))
            .map_err(|e| CliError::usage(format!("reading password: {e}")))?
    } else {
        return Err(CliError::usage(
            "no terminal: pass --password-stdin and pipe the password",
        ));
    };
    let project = a.scope_project.clone().or_else(|| ctx.project.clone());
    let body = compact(json!({
        "name": a.name,
        "password": password,
        "project": project,
        "ttl_secs": a.ttl,
    }));
    let url = a.url.trim_end_matches('/').to_owned();
    let anon = Client::new(&url, None, ctx.insecure)?;
    let resp = anon.post("/identity/tokens", &body).await?;
    let token = str_at(&resp, "/token");
    if token.is_empty() {
        return Err(CliError::Transport("server returned no token".into()));
    }
    let expires = str_at(&resp, "/expires_at");
    let principal = str_at(&resp, "/caller/principal_name");
    let token_project = str_at(&resp, "/caller/project");

    let name = ctx.profile_name.clone();
    ctx.config.set(
        &name,
        Profile {
            url: url.clone(),
            token: Some(token.clone()),
            project: project.clone(),
            insecure: ctx.insecure,
            principal: Some(principal.clone()),
            expires_at: Some(expires.clone()),
        },
    );
    ctx.config.current = Some(name.clone());
    ctx.config.save(&ctx.config_path)?;
    ctx.url = Some(url.clone());
    ctx.token = Some(token);

    match ctx.fmt.output {
        Output::Json | Output::Yaml => print_one(io.stdout, ctx.fmt, &resp)?,
        _ => {
            let scope = if token_project.is_empty() {
                "system scope".to_owned()
            } else {
                format!("project {}", short_id(&token_project))
            };
            writeln!(
                io.stdout,
                "logged in to {url} as {principal} ({scope}); profile \"{name}\" saved to {}; token expires {expires}",
                ctx.config_path.display()
            )?;
        }
    }
    Ok(())
}

fn profile(ctx: &mut Ctx, cmd: ProfileCmd, io: &mut Io<'_>) -> CliResult<()> {
    match cmd {
        ProfileCmd::List => {
            let current = ctx.config.current_name().to_owned();
            let rows: Vec<Vec<String>> = ctx
                .config
                .profiles
                .iter()
                .map(|(n, p)| {
                    vec![
                        if *n == current {
                            "*".into()
                        } else {
                            String::new()
                        },
                        n.clone(),
                        p.url.clone(),
                        p.principal.clone().unwrap_or_default(),
                        p.project.clone().unwrap_or_default(),
                        p.expires_at.clone().unwrap_or_default(),
                    ]
                })
                .collect();
            match ctx.fmt.output {
                Output::Json | Output::Yaml => {
                    let v = serde_json::to_value(&ctx.config.profiles)
                        .map_err(|e| CliError::Other(e.into()))?;
                    print_value(io.stdout, ctx.fmt, &v)?;
                }
                Output::Name => {
                    for n in ctx.config.profiles.keys() {
                        writeln!(io.stdout, "{n}")?;
                    }
                }
                _ => write!(
                    io.stdout,
                    "{}",
                    output::table(
                        &["", "NAME", "URL", "USER", "PROJECT", "EXPIRES"],
                        &rows,
                        ctx.fmt.no_headers
                    )
                )?,
            }
        }
        ProfileCmd::Use { name } => {
            if ctx.config.get(&name).is_none() {
                return Err(CliError::usage(format!("no profile named '{name}'")));
            }
            ctx.config.current = Some(name.clone());
            ctx.config.save(&ctx.config_path)?;
            writeln!(io.stdout, "current profile: {name}")?;
        }
        ProfileCmd::Show { name } => {
            let name = name.unwrap_or_else(|| ctx.profile_name.clone());
            let p = ctx
                .config
                .get(&name)
                .ok_or_else(|| CliError::usage(format!("no profile named '{name}'")))?;
            let mut shown = p.clone();
            shown.token = shown.token.map(|t| {
                if t.len() > 12 {
                    format!("{}…", &t[..12])
                } else {
                    "…".into()
                }
            });
            let mut v = serde_json::to_value(&shown).map_err(|e| CliError::Other(e.into()))?;
            if let Value::Object(m) = &mut v {
                m.insert("name".into(), json!(name));
            }
            print_one(io.stdout, ctx.fmt, &v)?;
        }
        ProfileCmd::Delete { name } => {
            if ctx.config.profiles.remove(&name).is_none() {
                return Err(CliError::usage(format!("no profile named '{name}'")));
            }
            if ctx.config.current.as_deref() == Some(&name) {
                ctx.config.current = ctx.config.profiles.keys().next().cloned();
            }
            ctx.config.save(&ctx.config_path)?;
            writeln!(io.stdout, "profile \"{name}\" deleted")?;
        }
    }
    Ok(())
}

async fn whoami(ctx: &Ctx, io: &mut Io<'_>) -> CliResult<()> {
    let c = ctx.client()?;
    if ctx.token.is_none() {
        return Err(CliError::Api {
            status: 401,
            code: "unauthorized".into(),
            message: "not logged in".into(),
            details: Default::default(),
        });
    }
    let v = c.get_value("/identity/tokens/self").await?;
    match ctx.fmt.output {
        Output::Table | Output::Wide => {
            let roles = v
                .pointer("/roles")
                .and_then(Value::as_array)
                .map(|a| output::join(a))
                .unwrap_or_default();
            let rows = vec![
                vec!["principal".into(), str_at(&v, "/principal_name")],
                vec!["id".into(), str_at(&v, "/principal")],
                vec!["project".into(), str_at(&v, "/project")],
                vec!["roles".into(), roles],
                vec!["system".into(), str_at(&v, "/system")],
                vec!["server".into(), c.base().to_owned()],
                vec!["profile".into(), ctx.profile_name.clone()],
            ];
            write!(
                io.stdout,
                "{}",
                output::table(&["FIELD", "VALUE"], &rows, true)
            )?;
        }
        Output::Name => writeln!(io.stdout, "{}", str_at(&v, "/principal_name"))?,
        _ => print_one(io.stdout, ctx.fmt, &v)?,
    }
    Ok(())
}

async fn modules(ctx: &Ctx, io: &mut Io<'_>) -> CliResult<()> {
    let c = ctx.client()?;
    let v = c.get_value("/").await?;
    let mods = v
        .pointer("/modules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match ctx.fmt.output {
        Output::Table | Output::Wide => {
            let rows: Vec<Vec<String>> = mods
                .iter()
                .map(|m| {
                    vec![
                        str_at(m, "/name"),
                        format!("/{}", str_at(m, "/prefix")),
                        str_at(m, "/version"),
                        str_at(m, "/summary"),
                    ]
                })
                .collect();
            write!(
                io.stdout,
                "{}",
                output::table(
                    &["MODULE", "PREFIX", "VERSION", "SUMMARY"],
                    &rows,
                    ctx.fmt.no_headers
                )
            )?;
        }
        Output::Name => {
            for m in &mods {
                writeln!(io.stdout, "{}", str_at(m, "/name"))?;
            }
        }
        _ => print_value(io.stdout, ctx.fmt, &Value::Array(mods))?,
    }
    Ok(())
}

async fn version(ctx: &Ctx, io: &mut Io<'_>) -> CliResult<()> {
    let client_v = env!("CARGO_PKG_VERSION");
    let server = match ctx.client() {
        Ok(c) => c.get_value("/").await.ok(),
        Err(_) => None,
    };
    match ctx.fmt.output {
        Output::Json | Output::Yaml => {
            let v = json!({
                "client": client_v,
                "server": server.as_ref().map(|s| json!({
                    "version": str_at(s, "/version"),
                    "url": str_at(s, "/public_url"),
                    "dev_mode": s.pointer("/dev_mode").cloned().unwrap_or(Value::Null),
                })),
            });
            print_value(io.stdout, ctx.fmt, &v)?;
        }
        _ => {
            writeln!(io.stdout, "pi {client_v}")?;
            match server {
                Some(s) => {
                    let dev = if s.pointer("/dev_mode") == Some(&Value::Bool(true)) {
                        " (dev mode)"
                    } else {
                        ""
                    };
                    writeln!(
                        io.stdout,
                        "pistack {} at {}{dev}",
                        str_at(&s, "/version"),
                        str_at(&s, "/public_url")
                    )?;
                }
                None => writeln!(io.stdout, "server: unreachable or not configured")?,
            }
        }
    }
    Ok(())
}

async fn api(ctx: &Ctx, a: ApiArgs, io: &mut Io<'_>) -> CliResult<()> {
    let c = ctx.client()?;
    let method = Method::from_bytes(a.method.to_uppercase().as_bytes())
        .map_err(|_| CliError::usage(format!("bad HTTP method '{}'", a.method)))?;
    let body = match &a.data {
        Some(d) => {
            let text = at_file(d)?;
            Some(
                serde_json::from_str::<Value>(&text)
                    .map_err(|e| CliError::usage(format!("request body is not JSON: {e}")))?,
            )
        }
        None => None,
    };
    let (status, v) = c.call(method, &a.path, body.as_ref()).await?;
    if v.is_null() {
        writeln!(
            io.stderr,
            "{} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        )?;
    } else {
        print_value(io.stdout, ctx.fmt, &v)?;
    }
    Ok(())
}

// ---- apply --------------------------------------------------------------------------

/// Spec fields that hold references: (`pointer`, noun, is_list). `apply`
/// resolves names in these to ids so manifests can say `network: lan`.
const REFS: &[(&str, &str, &str, bool)] = &[
    ("subnet", "/network", "network", false),
    ("port", "/network", "network", false),
    ("port", "/subnet", "subnet", false),
    ("port", "/device", "machine", false),
    ("port", "/security_groups", "secgroup", true),
    ("router", "/external_network", "network", false),
    ("router", "/interfaces", "subnet", true),
    ("fip", "/network", "network", false),
    ("fip", "/port", "port", false),
    ("snapshot", "/volume", "volume", false),
];

async fn resolve_refs(c: &Client, k: &KindDef, spec: &mut Value) -> CliResult<()> {
    for (noun, ptr, target, is_list) in REFS {
        if *noun != k.noun {
            continue;
        }
        let Some(slot) = spec.pointer_mut(ptr) else {
            continue;
        };
        let tk = kind(target);
        if *is_list {
            if let Value::Array(items) = slot {
                for it in items.iter_mut() {
                    if let Value::String(s) = it {
                        *s = resolve(c, tk, s).await?;
                    }
                }
            }
        } else if let Value::String(s) = slot {
            *s = resolve(c, tk, s).await?;
        }
    }
    if k.noun == "volume" {
        let src_kind = str_at(spec, "/source/kind");
        let tk = match src_kind.as_str() {
            "image" => Some(kind("image")),
            "snapshot" => Some(kind("snapshot")),
            "volume" => Some(kind("volume")),
            _ => None,
        };
        if let (Some(tk), Some(Value::String(s))) = (tk, spec.pointer_mut("/source/id")) {
            *s = resolve(c, tk, s).await?;
        }
    }
    Ok(())
}

fn read_manifest(io: &mut Io<'_>, path: &str) -> CliResult<String> {
    if path == "-" {
        let mut s = String::new();
        io.stdin.read_to_string(&mut s)?;
        Ok(s)
    } else {
        std::fs::read_to_string(path).map_err(|e| CliError::usage(format!("reading {path}: {e}")))
    }
}

fn parse_documents(text: &str) -> CliResult<Vec<Value>> {
    let mut docs = Vec::new();
    for doc in serde_yaml_ng::Deserializer::from_str(text) {
        let v: Value = serde::Deserialize::deserialize(doc)
            .map_err(|e| CliError::usage(format!("manifest is not valid YAML/JSON: {e}")))?;
        match v {
            Value::Null => {}
            Value::Array(items) => docs.extend(items),
            Value::Object(ref m) if m.contains_key("items") && !m.contains_key("kind") => {
                if let Some(Value::Array(items)) = m.get("items") {
                    docs.extend(items.iter().cloned());
                }
            }
            other => docs.push(other),
        }
    }
    Ok(docs)
}

async fn apply(ctx: &Ctx, a: ApplyArgs, io: &mut Io<'_>) -> CliResult<()> {
    let c = ctx.client()?;
    let mut docs = Vec::new();
    for f in &a.files {
        let text = read_manifest(io, f)?;
        docs.extend(parse_documents(&text)?);
    }
    if docs.is_empty() {
        return Err(CliError::usage("manifest contains no resources"));
    }
    for doc in docs {
        let kind_name = str_at(&doc, "/kind");
        let k = kinds::by_noun(&kind_name)
            .ok_or_else(|| CliError::usage(format!("unknown kind '{kind_name}' in manifest")))?;
        let name = str_at(&doc, "/name");
        if name.is_empty() && k.noun != "binding" {
            return Err(CliError::usage(format!("{} manifest has no name", k.noun)));
        }
        let mut spec = doc.get("spec").cloned().unwrap_or_else(|| json!({}));
        resolve_refs(&c, k, &mut spec).await?;
        let labels = doc.get("labels").cloned();
        let description = doc.get("description").cloned();
        let project = doc.get("project").cloned();

        let existing = if name.is_empty() {
            None
        } else {
            match c.get_value(&k.item_path(&name)).await {
                Ok(v) => Some(v),
                Err(e) if e.is_not_found() => None,
                Err(e) => return Err(e),
            }
        };
        match existing {
            None => {
                let mut body = Map::new();
                body.insert("name".into(), json!(name));
                if let Some(d) = description {
                    body.insert("description".into(), d);
                }
                if let Some(l) = labels {
                    body.insert("labels".into(), l);
                }
                if let Some(p) = project {
                    body.insert("project".into(), p);
                }
                if k.flat_create {
                    if let Value::Object(m) = compact(spec) {
                        body.extend(m);
                    }
                } else {
                    body.insert("spec".into(), compact(spec));
                }
                let v = c.post(k.path, &Value::Object(body)).await?;
                writeln!(
                    io.stdout,
                    "{} \"{}\" created ({})",
                    k.noun,
                    str_at(&v, "/name"),
                    short_id(&str_at(&v, "/id"))
                )?;
            }
            Some(cur) => match k.update {
                UpdateStyle::None => {
                    writeln!(
                        io.stdout,
                        "{} \"{name}\" exists (kind has no update)",
                        k.noun
                    )?;
                }
                UpdateStyle::Meta | UpdateStyle::Spec => {
                    let mut body = Map::new();
                    if let Some(d) = description {
                        body.insert("description".into(), d);
                    }
                    if let Some(l) = labels {
                        body.insert("labels".into(), l);
                    }
                    if k.update == UpdateStyle::Spec {
                        // Merge over the current spec so partial manifests work.
                        let mut merged = cur.get("spec").cloned().unwrap_or_else(|| json!({}));
                        merge(&mut merged, compact(spec));
                        body.insert("spec".into(), merged);
                    } else if k.flat_create {
                        if let Value::Object(m) = compact(spec) {
                            body.extend(m);
                        }
                    }
                    let rev = str_at(&cur, "/revision");
                    let path = k.item_path(&str_at(&cur, "/id"));
                    let v = c
                        .call(Method::PUT, &path, Some(&Value::Object(body)))
                        .await?
                        .1;
                    let _ = rev;
                    writeln!(
                        io.stdout,
                        "{} \"{}\" configured",
                        k.noun,
                        str_at(&v, "/name")
                    )?;
                }
            },
        }
    }
    Ok(())
}

fn merge(dst: &mut Value, src: Value) {
    match (dst, src) {
        (Value::Object(d), Value::Object(s)) => {
            for (k, v) in s {
                match d.get_mut(&k) {
                    Some(slot) if slot.is_object() && v.is_object() => merge(slot, v),
                    _ => {
                        d.insert(k, v);
                    }
                }
            }
        }
        (d, s) => *d = s,
    }
}

// ---- identity nouns -------------------------------------------------------------------

async fn project(ctx: &Ctx, cmd: ProjectCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("project");
    match cmd {
        ProjectCmd::List(a) => do_list(ctx, io, k, a).await,
        ProjectCmd::Show(a) => do_show(ctx, io, k, a).await,
        ProjectCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        ProjectCmd::Create(a) => {
            let quotas = parse_kv_u64(&a.quotas, "--quota")?;
            let spec = json!({
                "parent": opt(a.parent),
                "enabled": if a.disabled { Some(false) } else { None },
                "quotas": if quotas.is_empty() { Value::Null } else { Value::Object(quotas) },
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
    }
}

async fn user(ctx: &Ctx, cmd: UserCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("user");
    match cmd {
        UserCmd::List(a) => do_list(ctx, io, k, a).await,
        UserCmd::Show(a) => do_show(ctx, io, k, a).await,
        UserCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        UserCmd::Create(a) => {
            let password = if a.password_stdin {
                Some(io.read_line()?)
            } else {
                a.password.clone()
            };
            let spec = json!({
                "kind": match a.kind { PrincipalKind::User => "user", PrincipalKind::ServiceAccount => "service_account" },
                "email": opt(a.email),
                "password": password,
                "enabled": if a.disabled { Some(false) } else { None },
                "default_project": opt(a.default_project),
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
    }
}

async fn role(ctx: &Ctx, cmd: RoleCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("role");
    match cmd {
        RoleCmd::List(a) => do_list(ctx, io, k, a).await,
        RoleCmd::Show(a) => do_show(ctx, io, k, a).await,
        RoleCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        RoleCmd::Create(a) => {
            let meta = MetaArgs {
                description: a.description,
                labels: vec![],
            };
            let body = create_body(k, &a.name, &meta, json!({}))?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
    }
}

async fn binding(ctx: &Ctx, cmd: BindingCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("binding");
    match cmd {
        BindingCmd::List(a) => do_list(ctx, io, k, a).await,
        BindingCmd::Show(a) => do_show(ctx, io, k, a).await,
        BindingCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        BindingCmd::Create(a) => {
            let body = compact(json!({
                "principal": a.principal,
                "role": a.role,
                "project": opt(a.in_project),
            }));
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
    }
}

async fn key(ctx: &Ctx, cmd: KeyCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("key");
    match cmd {
        KeyCmd::List(a) => do_list(ctx, io, k, a).await,
        KeyCmd::Show(a) => do_show(ctx, io, k, a).await,
        KeyCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        KeyCmd::Create(a) => {
            let body = compact(json!({
                "name": a.name,
                "principal": opt(a.principal),
                "project": opt(a.scope_project),
                "expires_at": opt(a.expires_at),
            }));
            let c = ctx.client()?;
            let v = c.post(k.path, &body).await?;
            match ctx.fmt.output {
                Output::Table | Output::Wide => {
                    report(ctx, io, k, &v, "created")?;
                    writeln!(io.stdout, "secret (shown once): {}", str_at(&v, "/secret"))?;
                }
                Output::Name => writeln!(io.stdout, "{}", str_at(&v, "/secret"))?,
                _ => print_one(io.stdout, ctx.fmt, &v)?,
            }
            Ok(())
        }
    }
}

// ---- compute nouns -----------------------------------------------------------------------

async fn machine(ctx: &Ctx, cmd: MachineCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("machine");
    match cmd {
        MachineCmd::List(a) => do_list(ctx, io, k, a).await,
        MachineCmd::Show(a) => do_show(ctx, io, k, a).await,
        MachineCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        MachineCmd::Create(a) => {
            let mut keys = Vec::new();
            for s in &a.ssh_keys {
                keys.push(at_file(s)?.trim().to_owned());
            }
            let user_data = match &a.user_data {
                Some(u) => Some(at_file(u)?),
                None => None,
            };
            let spec = json!({
                "kind": match a.kind { MachineKind::Vm => "vm", MachineKind::Container => "container", MachineKind::Metal => "metal" },
                "size": a.size,
                "image": a.image,
                "networks": a.networks,
                "volumes": a.volumes,
                "ssh_keys": keys,
                "user_data": user_data,
                "host_selector": Value::Object(parse_kv(&a.host_selector, "--host-selector")?),
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            do_create(ctx, io, k, body, a.wait).await.map(|_| ())
        }
        MachineCmd::Start(a) => power(ctx, io, k, a, "start").await,
        MachineCmd::Stop(a) => power(ctx, io, k, a, "stop").await,
        MachineCmd::Reboot(a) => power(ctx, io, k, a, "reboot").await,
    }
}

async fn power(
    ctx: &Ctx,
    io: &mut Io<'_>,
    k: &KindDef,
    a: PowerArgs,
    action: &str,
) -> CliResult<()> {
    let v = do_action(ctx, io, k, &a.name, action, None).await?;
    if a.wait {
        let c = ctx.client()?;
        let want = if action == "stop" { "off" } else { "on" };
        let id = str_at(&v, "/id");
        let start = tokio::time::Instant::now();
        let mut last = String::new();
        loop {
            let cur = c.get_value(&k.item_path(&id)).await?;
            let p = str_at(&cur, "/status/power");
            if p != last {
                writeln!(io.stdout, "machine \"{}\": power {p}", a.name)?;
                last = p.clone();
            }
            if p == want {
                break;
            }
            if start.elapsed() > WAIT_TIMEOUT {
                return Err(CliError::usage(format!(
                    "timed out waiting for power {want}"
                )));
            }
            tokio::time::sleep(POLL).await;
        }
    }
    Ok(())
}

async fn size(ctx: &Ctx, cmd: SizeCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("size");
    match cmd {
        SizeCmd::List(a) => do_list(ctx, io, k, a).await,
        SizeCmd::Show(a) => do_show(ctx, io, k, a).await,
        SizeCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        SizeCmd::Create(a) => {
            let meta = MetaArgs {
                description: a.description,
                labels: vec![],
            };
            let spec = json!({
                "vcpus": a.vcpus,
                "memory_mib": a.memory_mib,
                "disk_gib": a.disk_gib,
                "gpus": a.gpus,
            });
            let body = create_body(k, &a.name, &meta, spec)?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
    }
}

async fn host(ctx: &Ctx, cmd: HostCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("host");
    match cmd {
        HostCmd::List(a) => do_list(ctx, io, k, a).await,
        HostCmd::Show(a) => do_show(ctx, io, k, a).await,
    }
}

// ---- network nouns -----------------------------------------------------------------------

async fn network(ctx: &Ctx, cmd: NetworkCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("network");
    match cmd {
        NetworkCmd::List(a) => do_list(ctx, io, k, a).await,
        NetworkCmd::Show(a) => do_show(ctx, io, k, a).await,
        NetworkCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        NetworkCmd::Create(a) => {
            // NetworkKind is internally tagged: {"type":"overlay"} | {"type":"provider","vlan":N,"physnet":".."}
            let kind_v = match a.kind {
                NetworkKind::Overlay => {
                    if a.vlan.is_some() {
                        return Err(CliError::usage("--vlan only applies to --kind provider"));
                    }
                    json!({"type": "overlay"})
                }
                NetworkKind::Provider => json!({
                    "type": "provider",
                    "vlan": a.vlan,
                    "physnet": a.physnet,
                }),
            };
            let spec = json!({
                "kind": kind_v,
                "mtu": a.mtu,
                "external": a.external,
                "dns_domain": opt(a.dns_domain),
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
    }
}

async fn subnet(ctx: &Ctx, cmd: SubnetCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("subnet");
    match cmd {
        SubnetCmd::List(a) => do_list(ctx, io, k, a).await,
        SubnetCmd::Show(a) => do_show(ctx, io, k, a).await,
        SubnetCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        SubnetCmd::Create(a) => {
            let c = ctx.client()?;
            let network = resolve(&c, kind("network"), &a.network).await?;
            let mut pools = Vec::new();
            for p in &a.pools {
                let (s, e) = p.split_once('-').ok_or_else(|| {
                    CliError::usage(format!("--pool must be START-END, got '{p}'"))
                })?;
                pools.push(json!({"start": s.trim(), "end": e.trim()}));
            }
            let spec = json!({
                "network": network,
                "cidr": a.cidr,
                "gateway": opt(a.gateway),
                "dns_nameservers": a.dns,
                "allocation_pools": pools,
                "enable_dhcp": !a.no_dhcp,
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
    }
}

async fn port(ctx: &Ctx, cmd: PortCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("port");
    match cmd {
        PortCmd::List(a) => do_list(ctx, io, k, a).await,
        PortCmd::Show(a) => do_show(ctx, io, k, a).await,
        PortCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        PortCmd::Create(a) => {
            let c = ctx.client()?;
            let spec = json!({
                "network": resolve(&c, kind("network"), &a.network).await?,
                "subnet": resolve_opt(&c, kind("subnet"), &a.subnet).await?,
                "fixed_ips": a.ips,
                "mac": a.mac.clone().unwrap_or_default(),
                "device": resolve_opt(&c, kind("machine"), &a.machine).await?,
                "security_groups": resolve_all(&c, kind("secgroup"), &a.secgroups).await?,
                "port_security": !a.no_port_security,
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
    }
}

async fn router(ctx: &Ctx, cmd: RouterCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("router");
    match cmd {
        RouterCmd::List(a) => do_list(ctx, io, k, a).await,
        RouterCmd::Show(a) => do_show(ctx, io, k, a).await,
        RouterCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        RouterCmd::Create(a) => {
            let c = ctx.client()?;
            let spec = json!({
                "external_network": resolve_opt(&c, kind("network"), &a.external_network).await?,
                "interfaces": resolve_all(&c, kind("subnet"), &a.subnets).await?,
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
        RouterCmd::SetInterfaces(a) => {
            let c = ctx.client()?;
            let subnets = resolve_all(&c, kind("subnet"), &a.subnets).await?;
            let path = format!("{}/interfaces", k.item_path(&a.name));
            let v = c.put(&path, &json!({ "subnets": subnets })).await?;
            report(
                ctx,
                io,
                k,
                &v,
                &format!("interfaces set ({} subnets)", a.subnets.len()),
            )
        }
    }
}

async fn secgroup(ctx: &Ctx, cmd: SecgroupCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("secgroup");
    match cmd {
        SecgroupCmd::List(a) => do_list(ctx, io, k, a).await,
        SecgroupCmd::Show(a) => do_show(ctx, io, k, a).await,
        SecgroupCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        SecgroupCmd::Create(a) => {
            let body = create_body(k, &a.name, &a.meta, json!({"rules": []}))?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
        SecgroupCmd::AddRule(a) => {
            let c = ctx.client()?;
            let port_range = match &a.port {
                None => Value::Null,
                Some(p) => {
                    let (lo, hi) = match p.split_once('-') {
                        Some((lo, hi)) => (lo.trim(), hi.trim()),
                        None => (p.trim(), p.trim()),
                    };
                    let lo: u16 = lo
                        .parse()
                        .map_err(|_| CliError::usage(format!("bad port '{p}'")))?;
                    let hi: u16 = hi
                        .parse()
                        .map_err(|_| CliError::usage(format!("bad port '{p}'")))?;
                    json!({"min": lo, "max": hi})
                }
            };
            // Remote is externally tagged: "any" | {"cidr": ".."} | {"group": "sg-.."}
            let remote = if let Some(cidr) = &a.cidr {
                json!({ "cidr": cidr })
            } else if let Some(g) = &a.remote_group {
                json!({ "group": resolve(&c, k, g).await? })
            } else {
                json!("any")
            };
            let rule = compact(json!({
                "direction": match a.direction { Direction::Ingress => "ingress", Direction::Egress => "egress" },
                "ethertype": match a.ethertype { EtherType::Ipv4 => "ipv4", EtherType::Ipv6 => "ipv6" },
                "protocol": match a.proto { Proto::Tcp => "tcp", Proto::Udp => "udp", Proto::Icmp => "icmp", Proto::Any => "any" },
                "port_range": port_range,
                "remote": remote,
            }));
            let path = format!("{}/rules", k.item_path(&a.name));
            let v = c.post(&path, &rule).await?;
            report(ctx, io, k, &v, "rule added")
        }
        SecgroupCmd::RemoveRule(a) => {
            let c = ctx.client()?;
            let path = format!("{}/rules/{}", k.item_path(&a.name), a.rule);
            let (_, v) = c.delete(&path).await?;
            report(ctx, io, k, &v, "rule removed")
        }
    }
}

async fn fip(ctx: &Ctx, cmd: FipCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("fip");
    match cmd {
        FipCmd::List(a) => do_list(ctx, io, k, a).await,
        FipCmd::Show(a) => do_show(ctx, io, k, a).await,
        FipCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        FipCmd::Create(a) => {
            let c = ctx.client()?;
            let spec = json!({
                "network": resolve(&c, kind("network"), &a.network).await?,
                "address": opt(a.address),
                "port": resolve_opt(&c, kind("port"), &a.port).await?,
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
        FipCmd::Associate(a) => {
            let c = ctx.client()?;
            let body = if let Some(machine) = &a.machine {
                json!({ "machine": resolve(&c, kind("machine"), machine).await? })
            } else if let Some(port) = &a.port {
                json!({ "port": resolve(&c, kind("port"), port).await? })
            } else {
                return Err(CliError::usage("associate needs --port or --machine"));
            };
            do_action(ctx, io, k, &a.name, "associate", Some(body))
                .await
                .map(|_| ())
        }
        FipCmd::Disassociate(a) => do_action(ctx, io, k, &a.name, "disassociate", None)
            .await
            .map(|_| ()),
    }
}

// ---- volume nouns ----------------------------------------------------------------------------

async fn volume(ctx: &Ctx, cmd: VolumeCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("volume");
    match cmd {
        VolumeCmd::List(a) => do_list(ctx, io, k, a).await,
        VolumeCmd::Show(a) => do_show(ctx, io, k, a).await,
        VolumeCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        VolumeCmd::Create(a) => {
            let c = ctx.client()?;
            // VolumeSource is adjacently tagged: {"kind":"image","id":"img-.."}; "none" has no id.
            let source = if let Some(i) = &a.image {
                json!({"kind": "image", "id": resolve(&c, kind("image"), i).await?})
            } else if let Some(s) = &a.snapshot {
                json!({"kind": "snapshot", "id": resolve(&c, kind("snapshot"), s).await?})
            } else if let Some(v) = &a.source_volume {
                json!({"kind": "volume", "id": resolve(&c, k, v).await?})
            } else {
                json!({"kind": "none"})
            };
            let spec = json!({
                "size_gib": a.size_gib,
                "volume_type": a.volume_type,
                "source": source,
                "bootable": a.bootable,
                "encrypted": a.encrypted,
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            do_create(ctx, io, k, body, a.wait).await.map(|_| ())
        }
        VolumeCmd::Attach(a) => {
            let c = ctx.client()?;
            let machine = resolve(&c, kind("machine"), &a.machine).await?;
            let body = compact(json!({ "machine": machine, "device": opt(a.device) }));
            do_action(ctx, io, k, &a.name, "attach", Some(body))
                .await
                .map(|_| ())
        }
        VolumeCmd::Detach(a) => do_action(ctx, io, k, &a.name, "detach", None)
            .await
            .map(|_| ()),
        VolumeCmd::Extend(a) => do_action(
            ctx,
            io,
            k,
            &a.name,
            "extend",
            Some(json!({ "size_gib": a.size_gib })),
        )
        .await
        .map(|_| ()),
    }
}

async fn snapshot(ctx: &Ctx, cmd: SnapshotCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("snapshot");
    match cmd {
        SnapshotCmd::List(a) => do_list(ctx, io, k, a).await,
        SnapshotCmd::Show(a) => do_show(ctx, io, k, a).await,
        SnapshotCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        SnapshotCmd::Create(a) => {
            let c = ctx.client()?;
            let spec = json!({ "volume": resolve(&c, kind("volume"), &a.volume).await? });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
    }
}

async fn voltype(ctx: &Ctx, cmd: VoltypeCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("voltype");
    match cmd {
        VoltypeCmd::List(a) => do_list(ctx, io, k, a).await,
        VoltypeCmd::Show(a) => do_show(ctx, io, k, a).await,
        VoltypeCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        VoltypeCmd::Create(a) => {
            let meta = MetaArgs {
                description: a.description,
                labels: vec![],
            };
            let spec = json!({
                "backend": a.backend,
                "params": Value::Object(parse_kv(&a.params, "--param")?),
            });
            let body = create_body(k, &a.name, &meta, spec)?;
            do_create(ctx, io, k, body, false).await.map(|_| ())
        }
    }
}

// ---- image ------------------------------------------------------------------------------------

async fn image(ctx: &Ctx, cmd: ImageCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("image");
    match cmd {
        ImageCmd::List(a) => do_list(ctx, io, k, a).await,
        ImageCmd::Show(a) => do_show(ctx, io, k, a).await,
        ImageCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        ImageCmd::Create(a) => {
            let spec = json!({
                "format": match a.format { ImageFormat::Raw => "raw", ImageFormat::Qcow2 => "qcow2", ImageFormat::Oci => "oci" },
                "os": opt(a.os),
                "arch": a.arch,
                "min_disk_gib": a.min_disk_gib,
                "min_ram_mib": a.min_ram_mib,
                "visibility": match a.visibility { Visibility::Private => "private", Visibility::Project => "project", Visibility::Public => "public" },
                "source_url": a.source_url.clone(),
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            let v = do_create(ctx, io, k, body, false).await?;
            let id = str_at(&v, "/id");
            let c = ctx.client()?;
            if let Some(file) = &a.file {
                upload(&c, ctx, io, k, &id, file).await?;
            } else if let Some(url) = &a.source_url {
                let path = format!("{}/import", k.item_path(&id));
                let v = c.post(&path, &json!({ "url": url })).await?;
                report(ctx, io, k, &v, "import started")?;
            }
            Ok(())
        }
        ImageCmd::Upload(a) => {
            let c = ctx.client()?;
            let id = resolve(&c, k, &a.name).await?;
            upload(&c, ctx, io, k, &id, &a.file).await
        }
        ImageCmd::Download(a) => {
            let c = ctx.client()?;
            let img = c.get_value(&k.item_path(&a.name)).await?;
            let id = str_at(&img, "/id");
            let out = a.out.unwrap_or_else(|| {
                std::path::PathBuf::from(format!(
                    "{}.{}",
                    str_at(&img, "/name"),
                    str_at(&img, "/spec/format")
                ))
            });
            let path = format!("{}/data", k.item_path(&id));
            let n = c.get_to_file(&path, &out).await?;
            writeln!(
                io.stdout,
                "image \"{}\" downloaded to {} ({n} bytes)",
                str_at(&img, "/name"),
                out.display()
            )?;
            Ok(())
        }
    }
}

async fn upload(
    c: &Client,
    ctx: &Ctx,
    io: &mut Io<'_>,
    k: &KindDef,
    id: &str,
    file: &std::path::Path,
) -> CliResult<()> {
    let path = format!("{}/data", k.item_path(id));
    writeln!(io.stderr, "uploading {}...", file.display())?;
    let v = c.put_file(&path, file).await?;
    let size = str_at(&v, "/status/size_bytes");
    report(ctx, io, k, &v, &format!("uploaded ({size} bytes)"))
}

async fn statefulset(ctx: &Ctx, cmd: StatefulsetCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("statefulset");
    match cmd {
        StatefulsetCmd::List(a) => do_list(ctx, io, k, a).await,
        StatefulsetCmd::Show(a) => do_show(ctx, io, k, a).await,
        StatefulsetCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        StatefulsetCmd::Create(a) => {
            let c = ctx.client()?;
            let mut spec = json!({
                "replicas": a.replicas,
                "template": {
                    "labels": { "app": a.name },
                    "spec": {
                        "containers": [{
                            "name": a.container.as_deref().filter(|s| !s.is_empty()).unwrap_or(&a.name),
                            "image": a.image,
                        }]
                    }
                }
            });
            if let Some(svc) = a.service_name {
                spec.as_object_mut()
                    .unwrap()
                    .insert("service_name".into(), json!(svc));
            }
            let body = create_body(k, &a.name, &a.meta, spec)?;
            let v = c.post(k.path, &body).await?;
            report(ctx, io, k, &v, "created")
        }
        StatefulsetCmd::Scale(a) => {
            let c = ctx.client()?;
            let id = resolve(&c, k, &a.name).await?;
            let v = c
                .post(
                    &format!("{}/{}/scale", k.path, id),
                    &json!({ "replicas": a.replicas }),
                )
                .await?;
            report(ctx, io, k, &v, "scaled")
        }
    }
}

async fn daemonset(ctx: &Ctx, cmd: DaemonsetCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("daemonset");
    match cmd {
        DaemonsetCmd::List(a) => do_list(ctx, io, k, a).await,
        DaemonsetCmd::Show(a) => do_show(ctx, io, k, a).await,
        DaemonsetCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        DaemonsetCmd::Create(a) => {
            let c = ctx.client()?;
            let spec = json!({
                "template": {
                    "labels": { "app": a.name },
                    "spec": {
                        "containers": [{
                            "name": a.container.as_deref().filter(|s| !s.is_empty()).unwrap_or(&a.name),
                            "image": a.image,
                        }]
                    }
                }
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            let v = c.post(k.path, &body).await?;
            report(ctx, io, k, &v, "created")
        }
    }
}

async fn autoscaler(ctx: &Ctx, cmd: AutoscalerCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("autoscaler");
    match cmd {
        AutoscalerCmd::List(a) => do_list(ctx, io, k, a).await,
        AutoscalerCmd::Show(a) => do_show(ctx, io, k, a).await,
        AutoscalerCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        AutoscalerCmd::Create(a) => {
            let c = ctx.client()?;
            let target_kind = match a.target_kind {
                ScaleTargetKindArg::Deployment => "Deployment",
                ScaleTargetKindArg::StatefulSet => "StatefulSet",
            };
            let spec = json!({
                "target_kind": target_kind,
                "target_name": a.target,
                "metric": a.metric,
                "target_value": a.target_value,
                "min_replicas": a.min_replicas,
                "max_replicas": a.max_replicas,
            });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            let v = c.post(k.path, &body).await?;
            report(ctx, io, k, &v, "created")
        }
    }
}

fn read_document(io: &mut Io<'_>, raw: &str) -> CliResult<String> {
    if let Some(path) = raw.strip_prefix('@') {
        return read_manifest(io, path);
    }
    if raw == "-" {
        return read_manifest(io, "-");
    }
    Ok(raw.to_owned())
}

async fn stack(ctx: &Ctx, cmd: StackCmd, io: &mut Io<'_>) -> CliResult<()> {
    let k = kind("stack");
    match cmd {
        StackCmd::List(a) => do_list(ctx, io, k, a).await,
        StackCmd::Show(a) => do_show(ctx, io, k, a).await,
        StackCmd::Delete(a) => do_delete(ctx, io, k, a).await,
        StackCmd::Create(a) => {
            let c = ctx.client()?;
            let document = read_document(io, &a.document)?;
            let spec = json!({ "name": a.name, "document": document });
            let body = create_body(k, &a.name, &a.meta, spec)?;
            let v = c.post(k.path, &body).await?;
            report(ctx, io, k, &v, "created")
        }
        StackCmd::Update(a) => {
            let c = ctx.client()?;
            let cur = c.get_value(&k.item_path(&a.name)).await?;
            let id = str_at(&cur, "/id");
            let rev = str_at(&cur, "/revision");
            let document = read_document(io, &a.document)?;
            let mut name = str_at(&cur, "/spec/name");
            if name.is_empty() {
                name = str_at(&cur, "/name");
            }
            if name.is_empty() {
                name = a.name.clone();
            }
            let body = json!({
                "spec": { "name": name, "document": document }
            });
            let v = if rev.is_empty() {
                c.put(&k.item_path(&id), &body).await?
            } else {
                c.put_if_match(&k.item_path(&id), &body, &rev).await?
            };
            report(ctx, io, k, &v, "updated")
        }
        StackCmd::Apply(a) => {
            let c = ctx.client()?;
            let id = resolve(&c, k, &a.name).await?;
            let v = c.post_empty(&format!("{}/{}/apply", k.path, id)).await?;
            report(ctx, io, k, &v, "applied")
        }
        StackCmd::Rollback(a) => {
            let c = ctx.client()?;
            let id = resolve(&c, k, &a.name).await?;
            let body = match a.revision {
                Some(r) => json!({ "revision": r }),
                None => json!({}),
            };
            let v = c
                .post(&format!("{}/{}/rollback", k.path, id), &body)
                .await?;
            report(ctx, io, k, &v, "rolled back")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_drops_nulls_recursively() {
        let v = compact(json!({"a": null, "b": {"c": null, "d": 1}, "e": [null, 2]}));
        assert_eq!(v, json!({"b": {"d": 1}, "e": [null, 2]}));
    }

    #[test]
    fn create_body_shapes() {
        let meta = MetaArgs {
            description: Some("d".into()),
            labels: vec!["env=prod".into()],
        };
        let flat = create_body(
            kind("project"),
            "p",
            &meta,
            json!({"enabled": false, "parent": null}),
        )
        .unwrap();
        assert_eq!(
            flat,
            json!({"name": "p", "description": "d", "labels": {"env": "prod"}, "enabled": false})
        );
        let nested = create_body(
            kind("machine"),
            "m",
            &meta,
            json!({"size": "s", "image": "i"}),
        )
        .unwrap();
        assert_eq!(nested["spec"], json!({"size": "s", "image": "i"}));
        assert!(create_body(
            kind("machine"),
            "m",
            &MetaArgs {
                description: None,
                labels: vec!["nope".into()]
            },
            json!({})
        )
        .is_err());
    }

    #[test]
    fn manifests_parse_multi_doc_and_lists() {
        let docs =
            parse_documents("kind: Machine\nname: a\n---\nkind: network\nname: b\n").unwrap();
        assert_eq!(docs.len(), 2);
        let docs = parse_documents(r#"[{"kind":"volume","name":"v"}]"#).unwrap();
        assert_eq!(docs.len(), 1);
        let docs = parse_documents("items:\n  - kind: size\n    name: s\n").unwrap();
        assert_eq!(docs[0]["kind"], "size");
        assert!(parse_documents("kind: [unclosed").is_err());
    }

    #[test]
    fn merge_is_deep() {
        let mut a = json!({"x": {"y": 1, "z": 2}, "k": [1]});
        merge(&mut a, json!({"x": {"z": 3}, "k": [2]}));
        assert_eq!(a, json!({"x": {"y": 1, "z": 3}, "k": [2]}));
    }
}
