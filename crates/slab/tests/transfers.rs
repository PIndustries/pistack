// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Volume transfers: an offer made on an available volume, taken by another
//! project with the secret, quota answered on both sides, expiry, cancel.

use gate::quota::{SNAPSHOTS, VOLUMES, VOLUMES_GIB};
use gate::{Project, ProjectSpec, ProjectStatus};
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::{kind, ApiError, Id, Meta, Phase};
use slab::{
    AllowAll, Client, Create, LocalFileBackend, ProjectQuota, SnapshotSpec, TransferPhase,
    VolumeSource, VolumeSpec,
};
use std::sync::Arc;

fn project(store: &Store, name: &str, limits: &[(&str, u64)]) -> Id {
    let mut spec = ProjectSpec::default();
    for (r, n) in limits {
        spec.quotas.insert((*r).to_string(), *n);
    }
    store
        .collection::<Project>()
        .create(Project {
            meta: Meta::new(kind::PROJECT, name, None),
            spec,
            status: ProjectStatus::default(),
        })
        .unwrap()
        .meta
        .id
}

fn used(store: &Store, p: &Id, resource: &str) -> u64 {
    store
        .collection::<Project>()
        .get(p)
        .unwrap()
        .status
        .usage
        .get(resource)
        .copied()
        .unwrap_or(0)
}

fn local(dir: &std::path::Path) -> Client {
    build(dir, Arc::new(crate::ProjectQuota))
}

/// A client that enforces no limits, for tests about moving rather than
/// about who may hold what.
fn unrestricted(dir: &std::path::Path) -> Client {
    build(dir, Arc::new(AllowAll))
}

fn build(dir: &std::path::Path, quota: Arc<dyn slab::QuotaCheck>) -> Client {
    let base = Client::local(dir).unwrap();
    let c = Client::new(base.store().clone(), base.backend().clone(), quota)
        .with_silo_root(base.silo_root().to_path_buf());
    c.seed_defaults().unwrap();
    c
}

async fn volume(c: &Client, p: &Id, name: &str, gib: u64) -> slab::Volume {
    let vol = make_volume(c, p, name, gib).await.unwrap();
    let vol = c.reconcile_volume(&vol.meta.id).await.unwrap().unwrap();
    assert_eq!(vol.status.phase, Phase::Ready);
    vol
}

fn blank() -> VolumeSpec {
    VolumeSpec {
        size_gib: 0,
        volume_type: "standard".into(),
        source: VolumeSource::None,
        bootable: false,
        encrypted: false,
    }
}

fn body(p: &Id, name: &str, gib: u64, make: impl FnOnce(u64) -> VolumeSpec) -> Create<VolumeSpec> {
    Create {
        name: name.into(),
        description: String::new(),
        labels: Default::default(),
        annotations: Default::default(),
        project: Some(p.clone()),
        spec: make(gib),
    }
}

async fn make_volume(c: &Client, p: &Id, name: &str, gib: u64) -> Result<slab::Volume, ApiError> {
    c.create_volume(body(p, name, gib, |size| VolumeSpec {
        size_gib: size,
        ..blank()
    }))
    .await
}

/// A volume created outside any project.
fn cloud_volume(name: &str) -> Create<VolumeSpec> {
    Create {
        name: name.into(),
        description: String::new(),
        labels: Default::default(),
        annotations: Default::default(),
        project: None,
        spec: VolumeSpec {
            size_gib: 1,
            ..blank()
        },
    }
}

async fn snapshot(c: &Client, vol: &slab::Volume, name: &str) -> slab::Snapshot {
    let s = c
        .create_snapshot(Create {
            name: name.into(),
            description: String::new(),
            labels: Default::default(),
            annotations: Default::default(),
            project: None,
            spec: SnapshotSpec {
                volume: vol.meta.id.clone(),
            },
        })
        .await
        .unwrap();
    c.reconcile_snapshot(&s.meta.id).await.unwrap();
    c.snapshots().get(&s.meta.id).unwrap()
}

fn body_g(p: &Id, name: &str, volume: &Id) -> Create<slab::BackupSpec> {
    Create {
        name: name.into(),
        description: String::new(),
        labels: Default::default(),
        annotations: Default::default(),
        project: Some(p.clone()),
        spec: slab::BackupSpec {
            volume: volume.clone(),
            target: "backups".into(),
        },
    }
}

#[tokio::test]
async fn a_volume_and_its_snapshots_move_to_the_project_that_holds_the_secret() {
    let dir = tempfile::tempdir().unwrap();
    let c = unrestricted(dir.path());
    let giver = project(c.store(), "giver", &[]);
    let taker = project(c.store(), "taker", &[]);
    let vol = volume(&c, &giver, "data", 4).await;
    let snap = snapshot(&c, &vol, "before").await;
    assert_eq!(snap.meta.project.as_ref(), Some(&giver));

    let offered = c.offer_volume(&vol.meta.id).unwrap();
    let id = offered.transfer.meta.id.clone();
    assert!(offered.secret.starts_with("vxt_"), "{}", offered.secret);
    assert_eq!(offered.transfer.status.phase, TransferPhase::Pending);
    assert_eq!(offered.transfer.meta.project.as_ref(), Some(&giver));
    // Nothing but this answer carries the secret.
    let stored = c.transfers().get(&id).unwrap();
    let dumped = serde_json::to_string(&stored).unwrap();
    assert!(!dumped.contains(&offered.secret), "{dumped}");

    let moved = c.accept_transfer(&id, &offered.secret, &taker).unwrap();
    assert_eq!(moved.meta.id, vol.meta.id);
    assert_eq!(moved.meta.project.as_ref(), Some(&taker));
    let snap = c.snapshots().get(&snap.meta.id).unwrap();
    assert_eq!(snap.meta.project.as_ref(), Some(&taker));

    let done = c.transfers().get(&id).unwrap();
    assert_eq!(done.status.phase, TransferPhase::Accepted);
    assert_eq!(done.status.accepted_by.as_ref(), Some(&taker));
    assert_eq!(c.volumes().list(Some(&giver)).unwrap().len(), 0);
    assert_eq!(c.volumes().list(Some(&taker)).unwrap().len(), 1);
    assert_eq!(c.snapshots().list(Some(&giver)).unwrap().len(), 0);

    // The bytes did not move: the backend reference is the same record.
    let backend = LocalFileBackend::new(dir.path());
    assert!(backend.volume_path(&moved).exists());
}

#[tokio::test]
async fn quota_is_answered_on_the_receiving_project() {
    let dir = tempfile::tempdir().unwrap();
    let c = local(dir.path());
    let giver = project(c.store(), "giver", &[]);
    let tight = project(c.store(), "tight", &[(VOLUMES_GIB, 2)]);
    let vol = volume(&c, &giver, "data", 4).await;
    let offered = c.offer_volume(&vol.meta.id).unwrap();

    let err = c
        .accept_transfer(&offered.transfer.meta.id, &offered.secret, &tight)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::QuotaExceeded);
    assert_eq!(err.details["resource"], VOLUMES_GIB);
    assert_eq!(err.details["requested"], 4);

    // Refused on the giver's side too, with nothing half-moved.
    assert_eq!(used(c.store(), &giver, VOLUMES), 1);
    assert_eq!(used(c.store(), &tight, VOLUMES), 0);
    assert_eq!(
        c.volumes().get(&vol.meta.id).unwrap().meta.project.as_ref(),
        Some(&giver)
    );
    let still_open = c.transfers().get(&offered.transfer.meta.id).unwrap();
    assert_eq!(still_open.status.phase, TransferPhase::Pending);
    assert!(still_open.spec.destination.is_none());

    // The offer survives the failure and can be taken by one with room.
    let roomy = project(c.store(), "roomy", &[]);
    c.accept_transfer(&offered.transfer.meta.id, &offered.secret, &roomy)
        .unwrap();
    assert_eq!(used(c.store(), &giver, VOLUMES), 0);
    assert_eq!(used(c.store(), &roomy, VOLUMES), 1);
    assert_eq!(used(c.store(), &roomy, VOLUMES_GIB), 4);
}

#[tokio::test]
async fn a_project_with_room_for_the_volume_but_not_its_snapshots_takes_neither() {
    let dir = tempfile::tempdir().unwrap();
    let c = local(dir.path());
    let giver = project(c.store(), "giver", &[]);
    let tight = project(c.store(), "tight", &[(SNAPSHOTS, 1)]);
    let vol = volume(&c, &giver, "data", 1).await;
    let first = snapshot(&c, &vol, "s1").await;
    let second = snapshot(&c, &vol, "s2").await;

    let offered = c.offer_volume(&vol.meta.id).unwrap();
    let err = c
        .accept_transfer(&offered.transfer.meta.id, &offered.secret, &tight)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::QuotaExceeded);
    assert_eq!(err.details["resource"], SNAPSHOTS);
    assert_eq!(err.details["requested"], 2);

    assert_eq!(
        c.volumes().get(&vol.meta.id).unwrap().meta.project.as_ref(),
        Some(&giver)
    );
    for s in [&first, &second] {
        assert_eq!(
            c.snapshots().get(&s.meta.id).unwrap().meta.project.as_ref(),
            Some(&giver),
            "snapshot {} came with the volume",
            s.meta.name
        );
    }
    assert_eq!(used(c.store(), &tight, VOLUMES), 0);
    assert_eq!(used(c.store(), &giver, SNAPSHOTS), 2);
}

#[tokio::test]
async fn the_giver_is_released_only_when_the_volume_leaves() {
    let dir = tempfile::tempdir().unwrap();
    let c = local(dir.path());
    // Room for exactly one 4 GiB volume, so the offer itself is the difference.
    let giver = project(c.store(), "giver", &[(VOLUMES_GIB, 4)]);
    let vol = volume(&c, &giver, "data", 4).await;
    let offered = c.offer_volume(&vol.meta.id).unwrap();
    assert_eq!(used(c.store(), &giver, VOLUMES_GIB), 4);

    // An open offer holds no extra room, and none is reserved for the taker:
    // a third volume is still refused while the offer is out.
    let err = make_volume(&c, &giver, "other", 1).await.unwrap_err();
    assert_eq!(err.details["resource"], VOLUMES_GIB);

    let taker = project(c.store(), "taker", &[(VOLUMES_GIB, 4)]);
    assert_eq!(used(c.store(), &taker, VOLUMES_GIB), 0);
    c.accept_transfer(&offered.transfer.meta.id, &offered.secret, &taker)
        .unwrap();
    assert_eq!(used(c.store(), &giver, VOLUMES_GIB), 0);
    assert_eq!(used(c.store(), &taker, VOLUMES_GIB), 4);

    // The giver can fill its freed room; the taker cannot take another.
    volume(&c, &giver, "replaced", 4).await;
    assert_eq!(used(c.store(), &giver, VOLUMES_GIB), 4);
    assert_eq!(used(c.store(), &taker, VOLUMES_GIB), 4);
}

#[tokio::test]
async fn a_wrong_secret_is_refused_without_saying_whether_the_id_exists() {
    let dir = tempfile::tempdir().unwrap();
    let c = unrestricted(dir.path());
    let giver = project(c.store(), "giver", &[]);
    let taker = project(c.store(), "taker", &[]);
    let vol = volume(&c, &giver, "data", 1).await;
    let offered = c.offer_volume(&vol.meta.id).unwrap();
    let id = offered.transfer.meta.id.clone();

    let wrong = c.accept_transfer(&id, "vxt_0000", &taker).unwrap_err();
    let never = Id::new("xfer");
    let unknown = c
        .accept_transfer(&never, offered.secret.clone().as_str(), &taker)
        .unwrap_err();

    // Same code, same message shape: one sentence naming the id it was given
    // and nothing about what was wrong with it.
    assert_eq!(wrong.code, ErrorCode::NotFound);
    assert_eq!(unknown.code, wrong.code);
    assert_eq!(
        wrong.message,
        format!("transfer {id} not found"),
        "{}",
        wrong.message
    );
    assert_eq!(unknown.message, format!("transfer {never} not found"));
    assert!(!wrong.message.contains("secret"));
    assert!(!unknown.message.contains("secret"));

    // The offer is untouched and still open to the one who knows the secret.
    assert_eq!(
        c.transfers().get(&id).unwrap().status.phase,
        TransferPhase::Pending
    );
    c.accept_transfer(&id, &offered.secret, &taker).unwrap();
}

#[tokio::test]
async fn one_offer_is_taken_once_and_the_second_caller_is_turned_away() {
    let dir = tempfile::tempdir().unwrap();
    let c = unrestricted(dir.path());
    let giver = project(c.store(), "giver", &[]);
    let first = project(c.store(), "first", &[]);
    let second = project(c.store(), "second", &[]);
    let vol = volume(&c, &giver, "data", 1).await;
    let offered = c.offer_volume(&vol.meta.id).unwrap();
    let id = offered.transfer.meta.id.clone();

    c.accept_transfer(&id, &offered.secret, &first).unwrap();
    let again = c
        .accept_transfer(&id, &offered.secret, &second)
        .unwrap_err();
    assert_eq!(again.code, ErrorCode::NotFound);
    assert_eq!(
        c.volumes().get(&vol.meta.id).unwrap().meta.project.as_ref(),
        Some(&first)
    );
}

#[tokio::test]
async fn the_owner_cancels_an_offer_before_anyone_takes_it() {
    let dir = tempfile::tempdir().unwrap();
    let c = unrestricted(dir.path());
    let giver = project(c.store(), "giver", &[]);
    let taker = project(c.store(), "taker", &[]);
    let vol = volume(&c, &giver, "data", 1).await;
    let offered = c.offer_volume(&vol.meta.id).unwrap();
    let id = offered.transfer.meta.id.clone();

    let cancelled = c.cancel_transfer(&id).unwrap();
    assert_eq!(cancelled.status.phase, TransferPhase::Closed);
    assert!(
        cancelled.status.message.contains("cancelled"),
        "{cancelled:?}"
    );
    assert_eq!(
        c.volumes().get(&vol.meta.id).unwrap().meta.project.as_ref(),
        Some(&giver)
    );

    let late = c.accept_transfer(&id, &offered.secret, &taker).unwrap_err();
    assert_eq!(late.code, ErrorCode::NotFound);

    // A closed offer is not cancelled again.
    assert_eq!(
        c.cancel_transfer(&id).unwrap_err().code,
        ErrorCode::Conflict
    );
}

#[tokio::test]
async fn an_offer_closes_itself_after_a_day() {
    let dir = tempfile::tempdir().unwrap();
    let c = unrestricted(dir.path());
    let giver = project(c.store(), "giver", &[]);
    let taker = project(c.store(), "taker", &[]);
    let vol = volume(&c, &giver, "data", 1).await;
    let offered = c.offer_volume(&vol.meta.id).unwrap();
    let id = offered.transfer.meta.id.clone();

    let open = c.transfers().get(&id).unwrap();
    let life = open
        .status
        .expires_at
        .unwrap()
        .signed_duration_since(open.meta.created_at);
    assert_eq!(life.num_hours(), 24, "{life}");

    // Past the window: the reconciler pass closes it, twice over is harmless.
    c.transfers()
        .modify(&id, |t| {
            t.status.expires_at = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
            Ok(())
        })
        .unwrap();
    assert_eq!(c.reap_expired_transfers().unwrap(), 1);
    assert_eq!(c.reap_expired_transfers().unwrap(), 0);
    let closed = c.transfers().get(&id).unwrap();
    assert_eq!(closed.status.phase, TransferPhase::Closed);
    assert_eq!(closed.status.message, "expired");

    // The secret still works as a string and still opens nothing.
    let err = c.accept_transfer(&id, &offered.secret, &taker).unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(
        c.volumes().get(&vol.meta.id).unwrap().meta.project.as_ref(),
        Some(&giver),
        "an expired offer moves nothing"
    );
}

#[tokio::test]
async fn a_cancelled_offer_frees_the_room_it_was_holding() {
    let dir = tempfile::tempdir().unwrap();
    let c = local(dir.path());
    let giver = project(c.store(), "giver", &[(VOLUMES, 1)]);
    let vol = volume(&c, &giver, "data", 1).await;
    let offered = c.offer_volume(&vol.meta.id).unwrap();

    // The room is still the giver's while the offer is out.
    assert_eq!(used(c.store(), &giver, VOLUMES), 1);
    assert_eq!(
        make_volume(&c, &giver, "other", 1).await.unwrap_err().code,
        ErrorCode::QuotaExceeded
    );

    // Giving up on the offer lets the volume be deleted, and the delete is
    // what returns the room.
    c.cancel_transfer(&offered.transfer.meta.id).unwrap();
    c.request_delete_volume(&vol.meta.id).unwrap();
    assert_eq!(used(c.store(), &giver, VOLUMES), 0);
    make_volume(&c, &giver, "other", 1).await.unwrap();
    assert_eq!(used(c.store(), &giver, VOLUMES), 1);
}

#[tokio::test]
async fn a_volume_that_is_not_available_cannot_be_offered() {
    let dir = tempfile::tempdir().unwrap();
    let c = unrestricted(dir.path());
    let giver = project(c.store(), "giver", &[]);

    // Attached.
    let vol = volume(&c, &giver, "plugged", 1).await;
    c.mark_attached(&vol.meta.id, &Id::new("mach"), None)
        .unwrap();
    let err = c.offer_volume(&vol.meta.id).unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("attached"), "{}", err.message);

    // An attach under way, with no machine settled on it yet.
    let half = volume(&c, &giver, "half", 1).await;
    c.store()
        .collection::<slab::Volume>()
        .modify(&half.meta.id, |v| {
            slab::set_condition(
                &mut v.status.conditions,
                "Attached",
                false,
                "Attaching",
                "in flight",
            );
            Ok(())
        })
        .unwrap();
    let err = c.offer_volume(&half.meta.id).unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("attach"), "{}", err.message);

    // A backup still being written.
    let busy = volume(&c, &giver, "busy", 1).await;
    c.create_backup(Create {
        name: "in-flight".into(),
        description: String::new(),
        labels: Default::default(),
        annotations: Default::default(),
        project: None,
        spec: slab::BackupSpec {
            volume: busy.meta.id.clone(),
            target: "backups".into(),
        },
    })
    .await
    .unwrap();
    let err = c.offer_volume(&busy.meta.id).unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("backup"), "{}", err.message);

    // A backup that is done writing, however it ended, is an object and no
    // obstacle. Reconciled without a bucket, it lands on `Error`.
    let done = volume(&c, &giver, "archived", 1).await;
    let b = c
        .create_backup(body_g(&giver, "written", &done.meta.id))
        .await
        .unwrap();
    c.reconcile_backup(&b.meta.id).await.unwrap();
    let b = c.backups().get(&b.meta.id).unwrap();
    assert_eq!(b.status.phase, Phase::Error, "{:?}", b.status);
    c.offer_volume(&done.meta.id).unwrap();

    // A volume of no project has nobody to give it from.
    let loose = c.create_volume(cloud_volume("loose")).await.unwrap();
    assert_eq!(
        c.offer_volume(&loose.meta.id).unwrap_err().code,
        ErrorCode::Unprocessable
    );
}

#[tokio::test]
async fn an_offered_volume_is_held_until_the_offer_is_withdrawn() {
    let dir = tempfile::tempdir().unwrap();
    let c = unrestricted(dir.path());
    let giver = project(c.store(), "giver", &[]);
    let taker = project(c.store(), "taker", &[]);
    let vol = volume(&c, &giver, "data", 1).await;
    let offered = c.offer_volume(&vol.meta.id).unwrap();
    let id = offered.transfer.meta.id.clone();

    // Someone holds a secret that can move this volume, so it is neither
    // plugged into a machine nor thrown away while that is true.
    let machine = Id::new("mach");
    let err = c.mark_attached(&vol.meta.id, &machine, None).unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("offered"), "{}", err.message);
    let err = c.request_delete_volume(&vol.meta.id).unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("transfer"), "{}", err.message);

    // Growing a volume that is not promised to anyone is still the owner's.
    let grown = c.extend_volume(&vol.meta.id, 2).await.unwrap();
    assert_eq!(grown.spec.size_gib, 2);
    c.reconcile_volume(&vol.meta.id).await.unwrap();
    let list = c.list_offers_for_volume(&vol.meta.id).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].meta.id, id);

    // Withdraw the offer and the volume is the owner's to use again.
    c.cancel_transfer(&id).unwrap();
    c.mark_attached(&vol.meta.id, &machine, None).unwrap();
    c.mark_detached(&vol.meta.id).unwrap();
    assert_eq!(
        c.accept_transfer(&id, &offered.secret, &taker)
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn one_volume_has_one_open_offer_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let c = unrestricted(dir.path());
    let giver = project(c.store(), "giver", &[]);
    let taker = project(c.store(), "taker", &[]);
    let vol = volume(&c, &giver, "data", 1).await;

    let a = c.offer_volume(&vol.meta.id).unwrap();
    let err = c.offer_volume(&vol.meta.id).unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    assert!(err.message.contains("already offered"), "{}", err.message);
    assert_eq!(c.list_offers_for_volume(&vol.meta.id).unwrap().len(), 1);

    // Withdraw it and a fresh offer, with a fresh secret, can be made.
    c.cancel_transfer(&a.transfer.meta.id).unwrap();
    let b = c.offer_volume(&vol.meta.id).unwrap();
    assert_ne!(a.secret, b.secret);
    assert_eq!(
        c.accept_transfer(&a.transfer.meta.id, &a.secret, &taker)
            .unwrap_err()
            .code,
        ErrorCode::NotFound,
        "the withdrawn secret opens nothing"
    );
    c.accept_transfer(&b.transfer.meta.id, &b.secret, &taker)
        .unwrap();
    assert_eq!(
        c.volumes().get(&vol.meta.id).unwrap().meta.project.as_ref(),
        Some(&taker)
    );
}

#[tokio::test]
async fn a_snapshot_of_another_volume_does_not_come_along() {
    let dir = tempfile::tempdir().unwrap();
    let c = unrestricted(dir.path());
    let giver = project(c.store(), "giver", &[]);
    let taker = project(c.store(), "taker", &[]);
    let vol = volume(&c, &giver, "data", 1).await;
    let other = volume(&c, &giver, "other", 1).await;
    let mine = snapshot(&c, &vol, "mine").await;
    let theirs = snapshot(&c, &other, "theirs").await;

    let offered = c.offer_volume(&vol.meta.id).unwrap();
    c.accept_transfer(&offered.transfer.meta.id, &offered.secret, &taker)
        .unwrap();
    assert_eq!(
        c.snapshots()
            .get(&mine.meta.id)
            .unwrap()
            .meta
            .project
            .as_ref(),
        Some(&taker)
    );
    assert_eq!(
        c.snapshots()
            .get(&theirs.meta.id)
            .unwrap()
            .meta
            .project
            .as_ref(),
        Some(&giver)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_projects_racing_for_one_offer_and_exactly_one_wins() {
    let dir = tempfile::tempdir().unwrap();
    let c = unrestricted(dir.path());
    let giver = project(c.store(), "giver", &[]);
    let vol = volume(&c, &giver, "data", 1).await;
    let offered = c.offer_volume(&vol.meta.id).unwrap();
    let id = offered.transfer.meta.id.clone();

    let mut tasks = Vec::new();
    for n in 0..6 {
        let (c, id, secret) = (c.clone(), id.clone(), offered.secret.clone());
        let taker = project(c.store(), &format!("taker{n}"), &[]);
        tasks.push(tokio::spawn(async move {
            c.accept_transfer(&id, &secret, &taker).is_ok()
        }));
    }
    let mut won = 0;
    for t in tasks {
        if t.await.unwrap() {
            won += 1;
        }
    }
    assert_eq!(won, 1, "one secret, one winner");
    let moved = c.volumes().get(&vol.meta.id).unwrap();
    let done = c.transfers().get(&id).unwrap();
    assert_eq!(done.status.phase, TransferPhase::Accepted);
    assert_eq!(done.status.accepted_by, moved.meta.project);
    assert_eq!(c.volumes().list(None).unwrap().len(), 1);
}
