# Decisions

Locked 2026-10-03. These close the open list in the 2026-10-02 design note.
The workshop module names, Apache-2.0 plus DCO, the redb `Backend` trait,
the in-process command queue, and a loom module were already decided.

## Hypervisor

libvirt/KVM is the first real machine runtime. Fleet and the lab nodes are
x86 KVM hosts, and that is the path that can boot a guest in Phase 1.
Cloud Hypervisor stays a later anvil runtime, already listed in Phase 3.
anvil keeps the mock runtime for machines that have no hypervisor.

## Blueprint language

A stack is a PiStack resource: YAML with the same `spec` / `status` shape as
every other object, applied with `pi apply`. The engine inside the binary is
that YAML. Other clients can call the API later.

## API

v1 speaks the PiStack API. A compatibility door for other clients waits until
a real migration needs it.

## First cloud

The first bootable set is `gate`, `core`, `forge`, `loom`, `anvil`, `fabric`,
`slab`, `mold`, and `bench`. One `pistack server` plus `pistack agent` on
the hosts is a cloud. That server is a one-voter cluster. Another server
discovers it on the private prefix, or is pointed at it with `--join`, and
replicates as a learner. A third server makes all three voters. Further
servers replicate as learners until an operator promotes them. Writes go to
the leader. This is not a requirement of the first demo, and a host agent
adds capacity rather than a control-plane vote.

## Containers

`loom` stores pods, labels, selectors, replica sets, deployments, and jobs
on the same spec and status records as every other module. v1 runs them
through `forge` and `anvil`. A second API in front of `loom` waits until a
pod runs on youki.

## Container runtime

anvil drives youki directly when the binary is on the host. There is no
containerd in the first runtime. Image bytes live in mold (and later silo).
When youki is absent, anvil uses the mock runtime and says so.

## Licence

Apache-2.0, DCO sign-off on every commit, PiStack trademark held by
P. Industries LLC. GPL-2.0-only would block the Apache-2.0 crates this
binary depends on (youki, pingora, kube-rs, cedar, opentelemetry).
