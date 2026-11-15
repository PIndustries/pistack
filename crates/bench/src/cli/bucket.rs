// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Object storage: `pi bucket` and `pi object`.
//!
//! A bucket is an ordinary resource and goes through the generic verbs. An
//! object is not: the object service keeps one record per key, holds the
//! bytes itself, and hangs its routes off the bucket
//! (`/silo/buckets/{bucket}/objects/{key}`). A put streams the file up and
//! a get streams it back, so object bytes are never held in memory.
//!
//! The kind table in `kinds.rs` also drives `apply`, and a bucket has no
//! update route, so this noun keeps its column set here instead of joining
//! that table.

use super::args::{
    BucketCmd, BucketCreate, ObjectCmd, ObjectDeleteArgs, ObjectGetArgs, ObjectListArgs,
    ObjectPutArgs,
};
use super::client::{CliError, CliResult};
use super::commands::{confirm, create_body, do_create, do_delete, do_list, do_show};
use super::kinds::{Column, KindDef, UpdateStyle};
use super::output::{print_one, scalar, short_id, str_at, table, Output};
use super::{Ctx, Io};
use anyhow::Context;
use serde_json::{json, Value};

fn name(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    str_at(v, "/name")
}
fn id(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    short_id(&str_at(v, "/id"))
}
fn full_id(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    str_at(v, "/id")
}
fn project(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    short_id(&str_at(v, "/project"))
}
fn description(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    str_at(v, "/description")
}
fn labels(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
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
fn phase(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    str_at(v, "/status/phase")
}
fn created(v: &Value, now: chrono::DateTime<chrono::Utc>) -> String {
    super::output::age(&str_at(v, "/created_at"), now)
}

/// A bucket holds the previous generation of a key when this is on.
fn versioning(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    match v.pointer("/spec/versioning").and_then(Value::as_bool) {
        Some(true) => "enabled".into(),
        Some(false) => "suspended".into(),
        None => String::new(),
    }
}

/// Where a bucket lives and how a bucket list prints. Ids start `bkt-`, so
/// `pi bucket show` takes a bucket id without a round trip.
pub static BUCKET: KindDef = KindDef {
    noun: "bucket",
    aliases: &["buckets"],
    path: "/silo/buckets",
    id_prefix: "bkt",
    update: UpdateStyle::None,
    flat_create: false,
    columns: &[
        Column {
            header: "NAME",
            get: name,
        },
        Column {
            header: "ID",
            get: id,
        },
        Column {
            header: "PHASE",
            get: phase,
        },
        Column {
            header: "VERSIONING",
            get: versioning,
        },
        Column {
            header: "DESCRIPTION",
            get: description,
        },
        Column {
            header: "AGE",
            get: created,
        },
    ],
    wide: &[
        Column {
            header: "FULL-ID",
            get: full_id,
        },
        Column {
            header: "PROJECT",
            get: project,
        },
        Column {
            header: "LABELS",
            get: labels,
        },
    ],
};

/// A listed object is the flat record the bucket's object route answers
/// with; a fetched one is a full resource. Each column reads whichever it
/// finds.
const OBJECT_COLUMNS: &[Column] = &[
    Column {
        header: "KEY",
        get: key_of,
    },
    Column {
        header: "SIZE",
        get: object_size,
    },
    Column {
        header: "VERSION",
        get: object_version,
    },
    Column {
        header: "CONTENT-TYPE",
        get: content_type_of,
    },
    Column {
        header: "SHA256",
        get: object_sha256,
    },
];

const OBJECT_WIDE: &[Column] = &[
    Column {
        header: "ID",
        get: object_id,
    },
    Column {
        header: "MD5",
        get: object_md5,
    },
];

fn key_of(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    let key = str_at(v, "/spec/key");
    if key.is_empty() {
        str_at(v, "/key")
    } else {
        key
    }
}

/// The key on a record, for callers with no timestamp to hand a column.
fn key(v: &Value) -> String {
    key_of(v, chrono::Utc::now())
}

/// The nested field, or the flat one when the record has no `status`.
fn pick(v: &Value, nested: &str, flat: &str) -> String {
    let got = str_at(v, nested);
    if got.is_empty() {
        str_at(v, flat)
    } else {
        got
    }
}

fn object_size(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    pick(v, "/status/size_bytes", "/size_bytes")
}
fn object_version(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    pick(v, "/status/version", "/version")
}
fn object_sha256(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    pick(v, "/status/sha256", "/sha256")
}
fn object_md5(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    pick(v, "/status/md5", "/md5")
}
fn content_type_of(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    str_at(v, "/spec/content_type")
}
fn object_id(v: &Value, _: chrono::DateTime<chrono::Utc>) -> String {
    short_id(&str_at(v, "/id"))
}

/// The object service is mounted at `/silo` and its `{bucket}` takes a
/// bucket name as readily as a bucket id.
fn object_path(bucket: &str, key: Option<&str>) -> String {
    match key {
        Some(k) => format!("/silo/buckets/{bucket}/objects/{k}"),
        None => format!("/silo/buckets/{bucket}/objects"),
    }
}

/// Percent-encode a key or a prefix, keeping the `/` that separates key
/// segments so a nested key stays one path.
fn esc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Catch the keys the service refuses before sending anything.
fn check_key(key: &str) -> CliResult<()> {
    let bad = || {
        CliError::usage(format!(
            "object key '{key}' must be a relative path without '.' or '..'"
        ))
    };
    if key.is_empty() {
        return Err(CliError::usage("object key is empty"));
    }
    if key.starts_with('/') || key.contains('\\') {
        return Err(bad());
    }
    if key
        .split('/')
        .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return Err(bad());
    }
    Ok(())
}

fn print_object_rows(ctx: &Ctx, io: &mut Io<'_>, items: &[Value]) -> CliResult<()> {
    if ctx.fmt.output == Output::Name {
        for o in items {
            writeln!(io.stdout, "{}", key(o))?;
        }
        return Ok(());
    }
    let cols: Vec<&Column> = if ctx.fmt.output == Output::Wide {
        OBJECT_COLUMNS.iter().chain(OBJECT_WIDE).collect()
    } else {
        OBJECT_COLUMNS.iter().collect()
    };
    let headers: Vec<&str> = cols.iter().map(|c| c.header).collect();
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|o| {
            let now = chrono::Utc::now();
            cols.iter().map(|c| (c.get)(o, now)).collect()
        })
        .collect();
    write!(io.stdout, "{}", table(&headers, &rows, ctx.fmt.no_headers))?;
    Ok(())
}

/// Objects in the shape `-o` asks for. The records are not resources of a
/// kind the console knows, so json and yaml are written here.
fn print_objects(ctx: &Ctx, io: &mut Io<'_>, items: &[Value]) -> CliResult<()> {
    match ctx.fmt.output {
        Output::Json => {
            let text = serde_json::to_string_pretty(items)
                .context("rendering objects as json")
                .map_err(CliError::from)?;
            writeln!(io.stdout, "{text}")?;
        }
        Output::Yaml => {
            let text = serde_yaml_ng::to_string(items).map_err(|e| CliError::Other(e.into()))?;
            write!(io.stdout, "{text}")?;
        }
        _ => print_object_rows(ctx, io, items)?,
    }
    Ok(())
}

pub async fn bucket(ctx: &Ctx, cmd: BucketCmd, io: &mut Io<'_>) -> CliResult<()> {
    match cmd {
        BucketCmd::List(a) => do_list(ctx, io, &BUCKET, a).await,
        BucketCmd::Show(a) => do_show(ctx, io, &BUCKET, a).await,
        BucketCmd::Delete(a) => do_delete(ctx, io, &BUCKET, a).await,
        BucketCmd::Create(a) => bucket_create(ctx, io, a).await,
    }
}

async fn bucket_create(ctx: &Ctx, io: &mut Io<'_>, a: BucketCreate) -> CliResult<()> {
    let spec = json!({ "versioning": a.versioning });
    let body = create_body(&BUCKET, &a.name, &a.meta, spec)?;
    do_create(ctx, io, &BUCKET, body, false).await.map(|_| ())
}

pub async fn object(ctx: &Ctx, cmd: ObjectCmd, io: &mut Io<'_>) -> CliResult<()> {
    match cmd {
        ObjectCmd::List(a) => object_list(ctx, io, a).await,
        ObjectCmd::Put(a) => object_put(ctx, io, a).await,
        ObjectCmd::Get(a) => object_get(ctx, io, a).await,
        ObjectCmd::Delete(a) => object_delete(ctx, io, a).await,
    }
}

async fn object_list(ctx: &Ctx, io: &mut Io<'_>, a: ObjectListArgs) -> CliResult<()> {
    let c = ctx.client()?;
    let v = c.get_value(&object_path(&a.bucket, None)).await?;
    let mut items = v
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // The route has no filter of its own, so the prefix is applied to what
    // comes back.
    if let Some(prefix) = &a.prefix {
        items.retain(|o| key(o).starts_with(prefix.as_str()));
    }
    print_objects(ctx, io, &items)
}

async fn object_put(ctx: &Ctx, io: &mut Io<'_>, a: ObjectPutArgs) -> CliResult<()> {
    check_key(&a.key)?;
    let c = ctx.client()?;
    let v = c
        .put_file(&object_path(&a.bucket, Some(&esc(&a.key))), &a.file)
        .await?;
    match ctx.fmt.output {
        Output::Table | Output::Wide => writeln!(
            io.stdout,
            "object \"{}\" uploaded ({} bytes)",
            key(&v),
            object_size(&v, chrono::Utc::now())
        )?,
        Output::Name => writeln!(io.stdout, "{}", key(&v))?,
        _ => print_one(io.stdout, ctx.fmt, &v)?,
    }
    Ok(())
}

async fn object_get(ctx: &Ctx, io: &mut Io<'_>, a: ObjectGetArgs) -> CliResult<()> {
    check_key(&a.key)?;
    let c = ctx.client()?;
    let path = object_path(&a.bucket, Some(&esc(&a.key)));
    match a.out {
        Some(out) => {
            let n = c.get_to_file(&path, &out).await?;
            writeln!(
                io.stdout,
                "object \"{}\" written to {} ({n} bytes)",
                a.key,
                out.display()
            )?;
        }
        // Without `-o` the bytes are the answer, streamed straight out.
        None => {
            c.get_to_writer(&path, io.stdout).await?;
        }
    }
    Ok(())
}

async fn object_delete(ctx: &Ctx, io: &mut Io<'_>, a: ObjectDeleteArgs) -> CliResult<()> {
    check_key(&a.key)?;
    let c = ctx.client()?;
    if !a.yes
        && !confirm(
            io,
            "delete",
            &format!("object \"{}\" in bucket \"{}\"", a.key, a.bucket),
        )?
    {
        writeln!(io.stderr, "aborted")?;
        return Ok(());
    }
    c.delete(&object_path(&a.bucket, Some(&esc(&a.key))))
        .await?;
    writeln!(io.stdout, "object \"{}\" deleted", a.key)?;
    Ok(())
}
