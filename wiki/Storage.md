# Storage

Four modules deal with storage: `slab` for block volumes, `mold` for images, `silo` for objects, and `shelf` for shared filesystems.

One fact shapes all of them today. Image files, local volume files, object data and share directories live on the control-plane server's local disk, under its data directory. They are not replicated between servers and not copied to hosts. The replicated store holds only the records. Object bytes are the exception: every server keeps a copy of what it lacks, and a put is confirmed only once another server holds the bytes. Until distribution is built for the rest, a setup that works end to end is one machine running both the server and the agent, or hosts that mount the server's data directory at the same path, or Ceph for volumes.

## Volumes

```sh
pi volume create data --size 20 --wait
pi volume attach data --machine web
pi volume extend data --size 40
pi volume detach data
```

A volume has a type, and the type names a backend. A new cloud has one type, `standard`, on the `local` backend.

```sh
pi voltype create fast --backend lvm --param vg=vg0 --param pool=pistack-thin
pi voltype create ceph --backend rbd --param pool=volumes --param monitors=10.0.0.21,10.0.0.22,10.0.0.23
pi volume create db --type fast --size 100
```

### Backends

| Backend | A volume is | Parameters | Needs |
| --- | --- | --- | --- |
| `local` | A sparse raw file, `<data_dir>/volumes/<id>.raw` | none | Nothing. `PISTACK_SLAB_DIR` moves the directory. |
| `lvm` | A thin logical volume | `vg` (required), `pool` (default `pistack-thin`) | The LVM tools, and a volume group and thin pool you created. |
| `rbd` | A Ceph RBD image | `pool` (default `volumes`), `user` (default `pistack`), `monitors` | The `rbd` command and a reachable cluster. |

Any backend also accepts `fs_type` (default `ext4`), the filesystem made on a blank volume when it is attached to a pod.

The backend commands run in the server process, not on the host. For `local` and `lvm` the volume is on the disk of the one server that made it, and the server then gives the agent a path on that server's own filesystem. That only works when the agent is on the same machine or sees the same path. `rbd` volumes are network disks and do not have this restriction. The server does not use `sudo` for LVM, so it needs permission to run the LVM tools itself.

### Which server holds a volume

A `local` or `lvm` volume records the node whose disk holds it as `status.home_node`. It attaches there and nowhere else: an attach asked for on another node is refused before anything is recorded, and the error names both nodes. An `rbd` volume has no home node.

A machine is placed where its volumes can attach by naming them at create:

```sh
pi machine create db --image ubuntu-24.04 --size m.2x4 --near-volume data
```

`--near-volume` is repeatable and attaches nothing. It only steers placement: the machine goes to the home node of every volume it names, and stays `pending` with a message naming the volume and the node when that node is not a usable host or the volumes sit on different nodes. A name that matches no volume, and an `rbd` volume, do not limit placement. Attach afterwards with `pi volume attach`.

A server that does not hold the bytes, a new leader after a failover, fails a snapshot, extend, backup or restore of such a volume and names the node that does. A delete there removes the record and leaves the file, so the volume can still be deleted once that node is gone. A volume made before the node was recorded is adopted by the server that finds its file.

### Attaching

A volume attaches to a virtual machine or to a pod.

- For a virtual machine under libvirt, a file volume is copied into libvirt's image directory at attach and copied back at detach. An RBD volume is attached as a network disk.
- For a pod, the agent turns the volume into a mounted directory under `<data_dir>/slab/mounts/<volume-id>`. A file is attached with `losetup`. A blank device is formatted; one that already has a filesystem is left alone.

A root agent runs `losetup`, `blkid`, `mkfs`, `mount` and `umount` directly. An agent that is not root needs `PISTACK_VOLFS_SUDO=1` and a sudoers rule for exactly those tools:

```text
# /etc/sudoers.d/pistack-volfs
pistack ALL=(root) NOPASSWD: /sbin/losetup, /sbin/blkid, /sbin/mkfs.ext4, /bin/mount, /bin/umount
```

Use the agent's user and the paths on your host. Mapping RBD images for pods works the same way with `PISTACK_RBD_SUDO=1` and the `rbd` command.

### Snapshots, clones and backups

```sh
pi snapshot create before-upgrade --volume data
pi volume create data2 --size 20 --snapshot before-upgrade
pi volume create data3 --size 20 --source-volume data
```

Snapshots are real on all three backends: a file copy, an LVM snapshot, or a protected RBD snapshot. A volume with snapshots cannot be deleted until they are.

A volume made from a snapshot or from another volume carries the source's data on all three backends: a file copy on `local`, a thin snapshot of the origin on `lvm`, and `rbd clone` or `rbd cp` on `rbd`.

`pi volume create --image NAME` fills a new volume from an image. It needs `qemu-img` on the server, and the image must be a disk image, not a container image.

Backups stream a volume into a `silo` bucket as one object, with a SHA-256 check. Restoring writes the bytes back: onto the file or the LVM device from byte 0, and on `rbd` through `rbd import` into a second image that replaces the volume's image only once the import has finished cleanly. A volume with snapshots is not restored over; restore into a new volume instead.

```sh
pi backup create nightly --volume data --bucket backups
pi backup list
pi backup show nightly
pi backup restore nightly --volume data2
pi backup delete nightly --yes
```

The bucket has to exist already: `pi bucket create backups`, or `pi api POST /silo/buckets -d '{"name":"backups"}'`. A new backup is `pending` while its bytes are uploaded and `ready` once they are stored; `--wait` holds until then. A restore needs a `ready` backup and returns only when the volume's bytes match the stored checksum. Without `--volume` it makes a new volume named after the backup, in the source volume's type and size.

Volumes can grow and cannot shrink. The `--encrypted` flag is stored on the volume and has no effect.

### Moving a volume to another project

A volume belongs to the project that created it. Handing one over is a transfer: the owner offers the volume and gets a transfer id and a secret, and someone in another project accepts with both. The volume and every snapshot taken from it arrive together, or nothing does.

There are no `pi` commands for it; the API is under `/volume`.

```sh
# in the giving project
pi api POST /volume/volumes/data/transfer
# -> { "transfer": { "id": "xfer-..." }, "secret": "vxt_..." }

# in the receiving project
pi api POST /volume/transfers/xfer-.../accept -d '{"secret":"vxt_..."}'

# until it is accepted, the giver can take the offer back
pi api DELETE /volume/transfers/xfer-...
```

The secret is returned once, with the offer, and is not stored: the record keeps only its hash. Nothing on any later read gives it back, so an offer cannot be repeated from the transfer record and `pi api GET /volume/transfers/xfer-...` is safe to script. A wrong secret, an id that was never issued and an offer that is closed all answer the same `not found`, so guessing at ids tells an attacker nothing.

Seeing a transfer follows the same line. The giving project reads its own offers; the receiving project reads one only after it has taken it, because until then nothing identifies it as the receiver except the secret. Whoever is handed the id and secret is whoever was meant to accept it.

Rules that hold it together:

- The volume must be sitting idle. An attached volume, one with an attach under way, or one with a backup still writing cannot be offered. A finished backup is no obstacle.
- While an offer is open the volume is held: it cannot be attached or deleted, so the volume that is accepted is the one that was offered. Cancel the transfer first.
- One open offer per volume. Cancelling and offering again issues a new secret; the old one then opens nothing.
- The receiving project is checked for the volume and its snapshots against its own limits before anything moves, and the giving project is released when it does. A project with room for the volume but not for its snapshots takes neither.
- An offer lasts 24 hours and then closes itself. Nothing moves when it does; the volume stays where it was.
- What arrives behaves like a volume created in the receiving project: it is listed there, and answers to its name and its id there. It stops answering to its name in the project that gave it. A name already used there for another volume refuses the accept, and the volume stays where it was.

## Images

```sh
pi image create ubuntu-24.04 --format qcow2 --os linux --file noble-server-cloudimg-amd64.img
pi image create debian-12 --format qcow2 --source-url https://example.org/debian-12-genericcloud-amd64.qcow2
pi image list
```

Formats are `raw`, `qcow2` and `oci`. An `oci` image is a container image stored as one tarball. There is no registry client: PiStack does not pull from Docker Hub or any other registry. You upload the tarball.

Importing from a URL accepts `http` and `https`, refuses loopback addresses and the metadata address, and stops at 8 GiB.

On upload the server computes a SHA-256 and inspects disk images with `qemu-img info` when it is installed. Images that reference a backing file are rejected.

Visibility is `private` (the owner's project), `project` (shared with named projects) or `public`.

Image data is kept on the server in `<data_dir>/images`, addressed by hash, so the same file uploaded twice is stored once. `PISTACK_MOLD_DIR` moves the directory.

### Images on hosts

There is no image distribution yet. When a machine starts, the server sends the agent the path of the image file on the server. An agent on another machine cannot open it unless that path is a shared mount. The libvirt runtime otherwise looks for a file it already has, named after the image, under `bases/` in its image directory, so pre-staging images on each host by hand also works. The youki runtime reads image data from its own `PISTACK_MOLD_DIR`.

## Object storage

`silo` serves an S3-compatible API at `/silo` on the API port. Addressing is path-style only: `http://server:8443/silo/<bucket>/<key>`. The region is `pistack`.

Objects are files under `<data_dir>/silo` on the server that took the put. Every other control-plane server copies them: it pulls what it lacks in the background, and a read that finds the record without the bytes fetches them from a peer and checks size and SHA-256 before serving. A put is not answered until one other server says it holds the bytes.

That wait is `[modules.silo] put_copies`, default 2 servers. With `1` a put is answered as soon as this server has the bytes. A put whose bytes no other server takes is answered `503` and says the object is stored on this server only; it is kept and copied later, so read before retrying. A cluster of one server waits for nobody.

Multipart staging is copied the same way. CreateMultipartUpload and UploadPart are answered only once another server holds the upload's metadata or the part, after checking the part's length and MD5, and every server pulls the uploads it lacks in the background. An upload in flight therefore survives a change of leader: the new leader has the parts and completes it. A part sent twice under the same number is settled by the copy written later.

Requests are authenticated with a bearer token, or with AWS Signature Version 4 using an access key from `pi access-key create`. See [Identity and access](Identity-and-access).

Buckets and objects also have native API and CLI commands: `pi bucket` for the buckets, `pi object` for the keys inside one. A get without `--out` streams the bytes to stdout, and a put streams the file up.

An S3 client can write through any server of the cluster. There is no header an S3 client can put on a request to ask to be carried, so a write that lands under `/silo` on a server that does not lead is taken to the leader by that server and the leader's answer handed back. The signature is checked at the leader against the address the client signed for. What a follower cannot hold in memory is refused in S3's own shape: `503` with `<Code>SlowDown</Code>`, naming the server that would take it, and the limit is 8 MiB. `pi` avoids the detour: before streaming a file over 8 MiB it asks the server it was given who leads, and sends the upload there.

### Buckets

A bucket belongs to the project that made it and names one region, `pistack`. `pi bucket create NAME` makes one, and `--versioning` makes it keep generations. Names share one namespace across the cloud: a name already taken in another project is refused. Through S3, `CreateBucket` answers `200` with a `Location` of `/<bucket>`, answers the same again for a name the caller already holds, and answers `409 BucketAlreadyExists` for one held elsewhere. A `CreateBucketConfiguration` body is checked against the payload hash the caller declared and then ignored; no location is applied.

A bucket can be deleted only when nothing is left in it: no key, no unfinished multipart upload, and no older generation or delete marker under any key. `pi bucket delete`, `DELETE /silo/buckets/{id}` and S3 `DeleteBucket` all refuse a bucket that still holds something with `BucketNotEmpty`.

`spec.versioning` is a flag. On a bucket with it, writing a key again keeps the previous bytes under `.blobs`, addressed by their SHA-256, and keeps the older record as one more generation of the key. A `versionId` on the wire is that record's generation number as a decimal string; there is no separate version identity.

A plain delete on a versioned bucket hides the key rather than taking it. It writes a delete marker as the key's newest generation: a record at the next number, size 0, no checksum, no bytes on disk. Everything under the marker stays where it is.

- A read of the key answers `404` with `x-amz-delete-marker` and the marker's version.
- An older generation still reads with `?versionId=`.
- Deleting the marker by its `versionId` brings the key back.
- A put under a marker restores the key at the next generation.
- A second plain delete, with a marker already current, takes the key and everything under it whole.
- `ListObjectVersions` lists markers among the versions, newest first within a key.

On a bucket without versioning a delete takes the key whole, as it always did. The same rule holds for the native `DELETE /silo/buckets/{id}/objects/{key}`, since both reach the key through one door. Volume backups and image storage write into buckets they make with versioning off, so their deletes are unaffected. The native surface reads a key's current generation only: generations, markers and the versioning flag are read and set on the S3 surface.

Usage is counted in buckets per project. There is no limit on object bytes: silo keeps no running total per bucket or project, so a project can be limited in buckets and not in what it stores in them.

### What is implemented

- `PutObject`, `GetObject`, `HeadObject`, `DeleteObject`, `DeleteObjects`
- `CopyObject`
- `CreateBucket`, `DeleteBucket`, `HeadBucket`
- `ListObjectsV2`, with prefix and delimiter
- `ListObjectVersions`: every generation of every key, with prefix, `max-keys`, and `key-marker` with `version-id-marker` to page inside a key
- `GetBucketVersioning` and `PutBucketVersioning` (`?versioning`). `Enabled` turns generations on, `Suspended` turns them off. The store has one flag, so a suspended bucket is answered as one that never had versioning: no `Status` element.
- Range reads on `GetObject`: `bytes=a-b`, `bytes=a-` and `bytes=-n`, answered with 206 and `Content-Range`. A range past the end is 416 `InvalidRange`; a multi-range or malformed header is ignored and the whole object is returned.
- Multipart upload: create, upload part, upload part copy, complete, abort, list parts, list uploads
- SigV4 in the header, presigned URLs (up to seven days), and chunked uploads

### What is not

- `ListBuckets`. List buckets with `pi bucket list`, or `pi api GET /silo/buckets`.
- `ListObjects` version 1.
- ACLs, bucket policies, lifecycle rules, tagging, CORS, server-side encryption.
- A `versionId` on a copy source. `CopyObject` and `UploadPartCopy` take the current generation only.
- Reading a delete marker as an object: a `GET` or `HEAD` naming a marker by `versionId` is `405 MethodNotAllowed`.
- SigV4A.

Limits: 5 GiB per single `PutObject`, parts of at least 5 MiB, 10,000 parts per upload, 1,000 keys per list page and per `DeleteObjects` request, and 8 MiB for a body carried by a server that does not lead. Unfinished multipart uploads are removed after seven days, counted from the upload's creation, by the leader. The object size limit, the multipart lifetime and the region can be changed in `[modules.silo]`; see the [Configuration reference](Configuration-reference).

An S3 client has to be set to path-style addressing, given `http://<server>/silo` as its endpoint and `pistack` as its region. It can create and remove buckets.

## Shared filesystems

`shelf` creates shares and exports them. See [pi share](CLI-reference#pi-share) for the commands; the API is under `/shelf`.

```sh
pi share create home --protocol nfs --size 200 --cidr 10.0.0.0/24 --mode read
pi share access list home
pi share access allow home --cidr 10.0.1.0/24 --mode write
```

| Backend | What happens | Needs |
| --- | --- | --- |
| NFS | A directory under `<data_dir>/shares` is created and exported to the share's CIDR. The server rewrites `/etc/exports.d/pistack.exports` and runs `exportfs -ra`. | An NFS server and `exportfs` on the control-plane machine. |
| CephFS | `ceph fs subvolume create` on the filesystem named in `PISTACK_CEPH_FS`. | The `ceph` command and that variable. |

Only the server that leads touches a share. A follower makes no directory, writes no export line and records no status; when it is promoted it reconciles every share on the next pass.

An export line carries the client CIDR and these options: `ro` or `rw`, `sync`, `no_subtree_check`, `all_squash`, and `anonuid`/`anongid` at one pair. `rw` is given only where the share allows that CIDR in mode write. Every client is squashed to that pair, so the share directory is owned by it and is mode `0770` with setgid: `rws` so a file one client writes is still writable by the next. The pair is nobody/nogroup, `65534`, unless `PISTACK_SHELF_SQUASH_UID` names another. Owner and mode are applied on every pass, so a directory that drifted is corrected without recreating the share. A server that cannot `chown` keeps exporting and says so in the log.

A server that is not root runs the export step through `sudo -n` and the helper named in `PISTACK_SHELF_HELPER`. Install `scripts/pistack-shelf-helper` as `/usr/local/sbin/pistack-shelf-helper`; its header has the sudoers line.

An NFS share with no CIDR, or a CephFS share when `ceph` or `PISTACK_CEPH_FS` is missing, stays `pending`. The size of a share is recorded and not enforced: an NFS share is a plain directory with no quota on it.

A ready NFS share says where to mount. `status.export` holds `<address>:<path>`, the address being the API host of the server that exports it. `pi share show` prints the record, with that field in it, and `pi share list -o wide` has an `EXPORT` column; the plain table has none. A share lives on that one server: its directory and its export line are local to it, and a change of leader does not move either. `PISTACK_EXPORT_ADDRESS` overrides the address for anything that runs `shelf` outside the server process.

`pi share access allow` and `deny` change one client rule. The API takes the whole spec, so the commands read the share, edit its list and write it back with `If-Match` on the revision they read; a share that moved in between is refused rather than overwritten.

## Where the data is

| Data | Location on the server | Override |
| --- | --- | --- |
| Records for everything | `<data_dir>/state.redb` (replicated) | |
| Local volumes and snapshots | `<data_dir>/volumes` | `PISTACK_SLAB_DIR` |
| Images | `<data_dir>/images` | `PISTACK_MOLD_DIR` |
| Objects | `<data_dir>/silo` | |
| Shares | `<data_dir>/shares` | `PISTACK_SHELF_DIR` |

`pistack store backup` and `pistack store restore` cover the first row only. Back up the directories with your usual tools.
