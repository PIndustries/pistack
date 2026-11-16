// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Volume backups: `pi backup list|show|create|delete|restore`.
//!
//! A backup holds one volume's bytes in a bucket. Creating one only records
//! the request; the copy happens in the background, so a fresh backup is
//! `pending` until its bytes are stored and `ready` afterwards. A restore
//! writes those bytes back: into a new volume, or into an existing one when
//! `--volume` names it.

use super::args::{DeleteArgs, ListArgs, MetaArgs, NameArg};
use super::client::{CliError, CliResult, Client};
use super::commands;
use super::kinds::{Column, KindDef, UpdateStyle};
use super::output::{age, print_list, print_one, scalar, short_id, str_at, Output};
use super::{Ctx, Io};
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};
use std::time::Duration;

const POLL: Duration = Duration::from_secs(1);
const WAIT_TIMEOUT: Duration = Duration::from_secs(600);

const NOUN: &str = "backup";

macro_rules! col {
    ($get:ident, $header:literal) => {
        Column {
            header: $header,
            get: $get,
        }
    };
}

/// Where backups live and how `pi backup list` prints them. Kept here rather
/// than in the shared kind table because this noun has a verb (`restore`) the
/// table cannot describe. A backup's metadata is not editable, so its update
/// style is `None`.
static BACKUP: KindDef = KindDef {
    noun: NOUN,
    aliases: &["backups"],
    path: "/volume/backups",
    id_prefix: "bak",
    update: UpdateStyle::None,
    flat_create: false,
    columns: &[
        col!(name, "NAME"),
        col!(short_id_col, "ID"),
        col!(phase, "PHASE"),
        col!(volume, "VOLUME"),
        col!(bucket, "BUCKET"),
        col!(size_gib, "SIZE-GIB"),
        col!(age_col, "AGE"),
    ],
    wide: &[
        col!(object, "OBJECT"),
        col!(parts, "PARTS"),
        col!(full_id, "FULL-ID"),
        col!(project, "PROJECT"),
        col!(labels, "LABELS"),
    ],
};

fn name(v: &Value, _: DateTime<Utc>) -> String {
    str_at(v, "/name")
}
fn short_id_col(v: &Value, _: DateTime<Utc>) -> String {
    short_id(&str_at(v, "/id"))
}
fn phase(v: &Value, _: DateTime<Utc>) -> String {
    str_at(v, "/status/phase")
}
fn volume(v: &Value, _: DateTime<Utc>) -> String {
    short_id(&str_at(v, "/spec/volume"))
}
fn bucket(v: &Value, _: DateTime<Utc>) -> String {
    // The target is a bucket id, which is what every other id column shows:
    // the short form, never the whole uuid.
    short_id(&str_at(v, "/spec/target"))
}
fn size_gib(v: &Value, _: DateTime<Utc>) -> String {
    str_at(v, "/status/size_gib")
}
fn age_col(v: &Value, now: DateTime<Utc>) -> String {
    age(&str_at(v, "/created_at"), now)
}
fn object(v: &Value, _: DateTime<Utc>) -> String {
    str_at(v, "/status/object_ref")
}
fn parts(v: &Value, _: DateTime<Utc>) -> String {
    str_at(v, "/status/part_count")
}
fn full_id(v: &Value, _: DateTime<Utc>) -> String {
    str_at(v, "/id")
}
fn project(v: &Value, _: DateTime<Utc>) -> String {
    short_id(&str_at(v, "/project"))
}
fn labels(v: &Value, _: DateTime<Utc>) -> String {
    v.pointer("/labels")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| format!("{k}={}", scalar(v)))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default()
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum BackupCmd {
    /// List backups.
    #[command(alias = "ls")]
    List(ListArgs),
    /// Show one by name or id.
    #[command(alias = "get")]
    Show(NameArg),
    /// Back up a volume.
    Create(BackupCreate),
    /// Delete by name or id.
    #[command(alias = "rm")]
    Delete(DeleteArgs),
    /// Write a backup's bytes back into a volume.
    Restore(BackupRestore),
}

#[derive(Args, Debug)]
pub struct BackupCreate {
    pub name: String,
    #[command(flatten)]
    pub meta: MetaArgs,
    /// Volume to back up (name or id).
    #[arg(long)]
    pub volume: String,
    /// Bucket to write the backup to (name or id).
    #[arg(long)]
    pub bucket: String,
    /// Wait until the backup is ready.
    #[arg(long)]
    pub wait: bool,
}

#[derive(Args, Debug)]
pub struct BackupRestore {
    /// Backup (name or id).
    pub name: String,
    /// Write into this volume (name or id) instead of making a new one. It
    /// must be ready and at least as large as the backup.
    #[arg(long)]
    pub volume: Option<String>,
}

pub async fn backup(ctx: &Ctx, cmd: BackupCmd, io: &mut Io<'_>) -> CliResult<()> {
    match cmd {
        BackupCmd::List(a) => list(ctx, io, a).await,
        BackupCmd::Show(a) => show(ctx, io, a).await,
        BackupCmd::Create(a) => create(ctx, io, a).await,
        BackupCmd::Delete(a) => del(ctx, io, a).await,
        BackupCmd::Restore(a) => restore(ctx, io, a).await,
    }
}

fn item_path(name_or_id: &str) -> String {
    BACKUP.item_path(name_or_id)
}

async fn list(ctx: &Ctx, io: &mut Io<'_>, a: ListArgs) -> CliResult<()> {
    let c = ctx.client()?;
    let items = list_all(&c, &a).await?;
    print_list(io.stdout, ctx.fmt, &BACKUP, &items)?;
    Ok(())
}

async fn show(ctx: &Ctx, io: &mut Io<'_>, a: NameArg) -> CliResult<()> {
    let c = ctx.client()?;
    let v = c.get_value(&item_path(&a.name)).await?;
    print_one(io.stdout, ctx.fmt, &v)?;
    Ok(())
}

async fn create(ctx: &Ctx, io: &mut Io<'_>, a: BackupCreate) -> CliResult<()> {
    let c = ctx.client()?;
    let volume = resolve_volume(&c, &a.volume).await?;
    // The bucket is resolved the way the volume is: the CLI sends the object
    // store's own id, never the word the operator typed. A bucket made in a
    // project is filed under `{kind}/{project}/{name}`, so a service that
    // resolves the typed name only in the record's own scope and the cloud
    // scope cannot find it, and every upload against such a backup fails.
    let bucket = resolve_bucket(&c, &a.bucket).await?;
    let mut spec = Map::new();
    spec.insert("volume".into(), json!(volume));
    spec.insert("target".into(), json!(bucket));
    let body = create_body(&a.name, &a.meta, Value::Object(spec))?;
    let v = c.post(BACKUP.path, &body).await?;
    report(ctx, io, &v, "created")?;
    if a.wait {
        wait_ready(&c, io, &str_at(&v, "/id")).await?;
    }
    Ok(())
}

async fn del(ctx: &Ctx, io: &mut Io<'_>, a: DeleteArgs) -> CliResult<()> {
    let c = ctx.client()?;
    if !a.yes && !commands::confirm(io, "delete", &format!("{NOUN} \"{}\"", a.name))? {
        writeln!(io.stderr, "aborted")?;
        return Ok(());
    }
    let (status, _) = c.delete(&item_path(&a.name)).await?;
    let verb = if status == reqwest::StatusCode::ACCEPTED {
        "deletion requested"
    } else {
        "deleted"
    };
    writeln!(io.stdout, "{NOUN} \"{}\" {verb}", a.name)?;
    if a.wait && status == reqwest::StatusCode::ACCEPTED {
        wait_gone(&c, io, &a.name).await?;
    }
    Ok(())
}

/// `POST /volume/backups/{id}/restore` takes the volume to write, by name or
/// id, and nothing else; an empty body makes a new volume named after the
/// backup. The restored volume comes back only once its bytes match what is
/// stored, so there is nothing to wait for here.
async fn restore(ctx: &Ctx, io: &mut Io<'_>, a: BackupRestore) -> CliResult<()> {
    let c = ctx.client()?;
    let path = format!("{}/restore", item_path(&a.name));
    let body = match &a.volume {
        Some(v) => json!({ "volume": resolve_volume(&c, v).await? }),
        None => json!({}),
    };
    let volume = c.post(&path, &body).await?;
    report_volume(ctx, io, &volume)
}

async fn resolve_volume(c: &Client, name_or_id: &str) -> CliResult<String> {
    let k = super::kinds::by_noun("volume").expect("volume is a known kind");
    if k.is_id(name_or_id) {
        return Ok(name_or_id.to_owned());
    }
    let v = c.get_value(&k.item_path(name_or_id)).await.map_err(|e| {
        if e.is_not_found() {
            CliError::usage(format!("volume '{name_or_id}' not found"))
        } else {
            e
        }
    })?;
    Ok(str_at(&v, "/id"))
}

/// The bucket's own id, so a backup's `spec.target` names a row and not a
/// word. A name the caller cannot read is reported by this command, in these
/// words, rather than by whichever route happened to answer: `GET /silo/buckets`
/// answers `forbidden` with a bucket id in the message for a caller that named
/// a name, which reads like a server bug and hides the operator's own typo.
async fn resolve_bucket(c: &Client, name_or_id: &str) -> CliResult<String> {
    let k = &super::bucket::BUCKET;
    if k.is_id(name_or_id) {
        return Ok(name_or_id.to_owned());
    }
    let missing = CliError::usage(format!("bucket '{name_or_id}' not found"));
    let v = c
        .get_or_none(&k.item_path(name_or_id))
        .await?
        .ok_or(missing)?;
    Ok(str_at(&v, "/id"))
}

fn create_body(name: &str, meta: &MetaArgs, spec: Value) -> CliResult<Value> {
    let mut body = Map::new();
    body.insert("name".into(), json!(name));
    if let Some(d) = &meta.description {
        body.insert("description".into(), json!(d));
    }
    if !meta.labels.is_empty() {
        body.insert("labels".into(), json!(parse_labels(&meta.labels)?));
    }
    body.insert("spec".into(), spec);
    Ok(Value::Object(body))
}

fn parse_labels(pairs: &[String]) -> CliResult<Map<String, Value>> {
    let mut m = Map::new();
    for p in pairs {
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| CliError::usage(format!("--label must be key=value, got '{p}'")))?;
        m.insert(k.trim().to_owned(), Value::String(v.trim().to_owned()));
    }
    Ok(m)
}

async fn list_all(c: &Client, a: &ListArgs) -> CliResult<Vec<Value>> {
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
            BACKUP.path.to_owned()
        } else {
            format!("{}?{}", BACKUP.path, q.join("&"))
        };
        let page: pistack_types::Page<Value> = c.get(&path).await?;
        let got = page.items.len();
        items.extend(page.items);
        match page.next {
            Some(n) if got > 0 && Some(&n) != after.as_ref() => after = Some(n),
            _ => break,
        }
    }
    Ok(items)
}

fn report(ctx: &Ctx, io: &mut Io<'_>, v: &Value, verb: &str) -> CliResult<()> {
    match ctx.fmt.output {
        Output::Table | Output::Wide => {
            let id = str_at(v, "/id");
            if id.is_empty() {
                writeln!(io.stdout, "{NOUN} \"{}\" {verb}", str_at(v, "/name"))?;
            } else {
                writeln!(
                    io.stdout,
                    "{NOUN} \"{}\" {verb} ({})",
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

/// A restore ends with a volume, so it is reported as one.
fn report_volume(ctx: &Ctx, io: &mut Io<'_>, v: &Value) -> CliResult<()> {
    match ctx.fmt.output {
        Output::Table | Output::Wide => writeln!(
            io.stdout,
            "volume \"{}\" restored ({})",
            str_at(v, "/name"),
            short_id(&str_at(v, "/id"))
        )?,
        Output::Name => writeln!(io.stdout, "{}", str_at(v, "/name"))?,
        _ => print_one(io.stdout, ctx.fmt, v)?,
    }
    Ok(())
}

/// Poll until the backup is `ready`, printing each phase change. An `error`
/// phase is an error out; the reason is in its conditions.
async fn wait_ready(c: &Client, io: &mut Io<'_>, id: &str) -> CliResult<Value> {
    let start = tokio::time::Instant::now();
    let mut last = String::new();
    loop {
        let v = c.get_value(&item_path(id)).await?;
        let phase = str_at(&v, "/status/phase");
        if phase != last {
            match failure(&v) {
                Some(why) => writeln!(
                    io.stdout,
                    "{NOUN} \"{}\": {phase} ({why})",
                    str_at(&v, "/name")
                )?,
                None => writeln!(io.stdout, "{NOUN} \"{}\": {phase}", str_at(&v, "/name"))?,
            }
            last = phase.clone();
        }
        match phase.as_str() {
            "ready" => return Ok(v),
            "error" => {
                return Err(CliError::usage(format!(
                    "{NOUN} \"{}\" failed: {}",
                    str_at(&v, "/name"),
                    failure(&v).unwrap_or_default()
                )))
            }
            _ => {}
        }
        if start.elapsed() > WAIT_TIMEOUT {
            return Err(CliError::usage(format!(
                "timed out waiting for {NOUN} \"{}\" (last phase: {phase})",
                str_at(&v, "/name")
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

async fn wait_gone(c: &Client, io: &mut Io<'_>, name: &str) -> CliResult<()> {
    let start = tokio::time::Instant::now();
    let mut last = String::new();
    loop {
        match c.get_value(&item_path(name)).await {
            Ok(v) => {
                let phase = str_at(&v, "/status/phase");
                if phase != last {
                    writeln!(io.stdout, "{NOUN} \"{name}\": {phase}")?;
                    last = phase;
                }
            }
            Err(e) if e.is_not_found() => {
                writeln!(io.stdout, "{NOUN} \"{name}\": gone")?;
                return Ok(());
            }
            Err(e) => return Err(e),
        }
        if start.elapsed() > WAIT_TIMEOUT {
            return Err(CliError::usage(format!(
                "timed out waiting for {NOUN} \"{name}\" to go away"
            )));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// The failed upload's message, from the condition the server records.
fn failure(v: &Value) -> Option<String> {
    v.pointer("/status/conditions")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find(|c| {
                c.get("status").and_then(Value::as_bool) == Some(false)
                    && !c
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .is_empty()
            })
        })
        .map(|c| {
            c.get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        })
}
