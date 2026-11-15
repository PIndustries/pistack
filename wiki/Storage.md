# Storage

Four modules deal with storage: `slab` for block volumes, `mold` for images, `silo` for objects, and `shelf` for shared filesystems.

One fact shapes all of them today. Image files, local volume files, object data and share directories live on the control-plane server's local disk, under its data directory. They are not replicated between servers and not copied to hosts. The replicated store holds only the records. Until distribution is built, a setup that works end to end is one machine running both the server and the agent, or hosts that mount the server's data directory at the same path, or Ceph for volumes.

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

The backend commands run in the server process, not on the host. For `local` and `lvm` the server then gives the agent a path on the server's own filesystem. That only works when the agent is on the same machine or sees the same path. `rbd` volumes are network disks and do not have this restriction. The server does not use `sudo` for LVM, so it needs permission to run the LVM tools itself.

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

Creating a volume from a snapshot or from another volume copies the data on the `local` backend only. On `lvm` and `rbd` the new volume comes out blank. Do not rely on restore-from-snapshot or clone on those backends yet.

`pi volume create --image NAME` fills a new volume from an image. It needs `qemu-img` on the server, and the image must be a disk image, not a container image.

Backups stream a volume into a `silo` bucket as one object, with a SHA-256 check. Restoring a backup works for `local` and `lvm` volumes and not for `rbd`.

```sh
pi backup create nightly --volume data --bucket backups
pi backup list
pi backup show nightly
pi backup restore nightly --volume data2
pi backup delete nightly --yes
```

The bucket has to exist already: `pi api POST /silo/buckets -d '{"name":"backups"}'`. A new backup is `pending` while its bytes are uploaded and `ready` once they are stored; `--wait` holds until then. A restore needs a `ready` backup and returns only when the volume's bytes match the stored checksum. Without `--volume` it makes a new volume named after the backup, in the source volume's type and size.

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

Objects are files on the server's disk under `<data_dir>/silo`. There is one copy. Nothing is replicated to other servers.

Requests are authenticated with a bearer token, or with AWS Signature Version 4 using an access key from `pi access-key create`. See [Identity and access](Identity-and-access).

### What is implemented

- `PutObject`, `GetObject`, `HeadObject`, `DeleteObject`, `DeleteObjects`
- `CopyObject`
- `ListObjectsV2`, with prefix and delimiter
- Range reads on `GetObject`: `bytes=a-b`, `bytes=a-` and `bytes=-n`, answered with 206 and `Content-Range`. A range past the end is 416 `InvalidRange`; a multi-range or malformed header is ignored and the whole object is returned.
- Multipart upload: create, upload part, upload part copy, complete, abort, list parts, list uploads
- SigV4 in the header, presigned URLs (up to seven days), and chunked uploads

### What is not

- Bucket operations through S3. `CreateBucket`, `DeleteBucket` and `ListBuckets` are not there. Create buckets through the native API: `pi api POST /silo/buckets -d '{"name":"backups"}'`.
- `ListObjects` version 1.
- ACLs, bucket policies, lifecycle rules, tagging, CORS, server-side encryption.
- Reading old versions. A bucket can keep earlier generations of an object, but there is no call to fetch one.
- SigV4A.

Limits: 5 GiB per single `PutObject`, parts of at least 5 MiB, 10,000 parts per upload, 1,000 keys per list page and per `DeleteObjects` request. Unfinished multipart uploads are removed after seven days. The object size limit, the multipart lifetime and the region can be changed in `[modules.silo]`; see the [Configuration reference](Configuration-reference).

An S3 client has to be set to path-style addressing, given `http://<server>/silo` as its endpoint and `pistack` as its region, and must not try to create the bucket itself.

## Shared filesystems

`shelf` creates shares and exports them. There are no `pi` commands for it; the API is under `/shelf`.

| Backend | What happens | Needs |
| --- | --- | --- |
| NFS | A directory under `<data_dir>/shares` is created and exported to the share's CIDR. The server rewrites `/etc/exports.d/pistack.exports` and runs `exportfs -ra`. | An NFS server and `exportfs` on the control-plane machine. |
| CephFS | `ceph fs subvolume create` on the filesystem named in `PISTACK_CEPH_FS`. | The `ceph` command and that variable. |

A server that is not root runs the export step through `sudo -n` and the helper named in `PISTACK_SHELF_HELPER`. Install `scripts/pistack-shelf-helper` as `/usr/local/sbin/pistack-shelf-helper`; its header has the sudoers line.

An NFS share with no CIDR, or a CephFS share when `ceph` or `PISTACK_CEPH_FS` is missing, stays `pending`. The size of an NFS share is recorded and not enforced.

## Where the data is

| Data | Location on the server | Override |
| --- | --- | --- |
| Records for everything | `<data_dir>/state.redb` (replicated) | |
| Local volumes and snapshots | `<data_dir>/volumes` | `PISTACK_SLAB_DIR` |
| Images | `<data_dir>/images` | `PISTACK_MOLD_DIR` |
| Objects | `<data_dir>/silo` | |
| Shares | `<data_dir>/shares` | `PISTACK_SHELF_DIR` |

`pistack store backup` and `pistack store restore` cover the first row only. Back up the directories with your usual tools.
