# VPCs

Status: proposed, 2026-11-14. Nothing here is built yet.

## Why

A project today holds loose parts: networks, subnets, routers, and the
interfaces that join them. Getting two subnets to talk means making a router
and attaching both by hand. Most people want one thing: a private space with
a few subnets in it that can already reach each other, a way out, and nothing
else able to reach in.

A VPC is that thing. It is also the unit later work hangs on: peering, a
load balancer address that lives on the tenant side, names that answer with
several addresses, and one day a link between clusters.

## What a VPC is

A VPC is a routed private address space inside a project.

- A project may have several. They do not route to each other.
- A VPC has one or more address blocks. Its subnets are cut from them.
- Every subnet in a VPC can reach every other subnet in it. No one makes a
  router; the VPC owns one.
- Two VPCs may use the same addresses. They never meet unless peered, and
  peering refuses blocks that overlap.
- A VPC with an external network set can reach out through it, and its
  machines can hold floating IPs from it.

The application sees none of this. It asks for a machine or a workload in a
VPC and gets an address that can reach its neighbours.

## Shape

```yaml
kind: vpc
meta:
  name: prod
  project: acme
spec:
  cidrs: [10.20.0.0/16]          # one or more; IPv4 or IPv6; no overlap with each other
  external_network: public       # optional; where traffic leaves and floating IPs come from
  dns_domain: prod.acme.internal # optional; handed to subnets that set none
  routes: []                     # optional static routes: destination, nexthop
status:
  phase: ready
  router: rtr-...                # the router this VPC owns
  subnets: 3
  message: ""
```

A network gains one field:

```yaml
kind: network
spec:
  vpc: vpc-...                   # optional; set at create, never changed
```

Nothing else changes shape. Subnets, ports, security groups and floating IPs
are what they are today.

## Rules

1. Creating a VPC creates its router. The router carries the VPC's id and is
   shown as owned. A direct change to an owned router, or deleting it, is
   refused with a line that names the VPC.
2. A subnet on a network that belongs to a VPC must sit inside one of the
   VPC's blocks and must not overlap another subnet in the same VPC. It is
   refused otherwise, before anything is made.
3. Such a subnet gets a router interface as part of its own create, and loses
   it as part of its own delete. Either both happen or neither.
4. A network in a VPC cannot be external and cannot be a provider network in
   the first version.
5. `cidrs` may grow. A block can be removed only when no subnet uses it.
6. `external_network` may be set, changed or cleared. Clearing it is refused
   while a floating IP in the VPC depends on it.
7. A VPC is deleted only when it has no subnets. Its router goes with it.
8. A VPC, its networks and its subnets live in one project.

## API

Beside the existing network routes:

```
GET    /network/vpcs
POST   /network/vpcs
GET    /network/vpcs/{id}
PUT    /network/vpcs/{id}
DELETE /network/vpcs/{id}

GET    /network/vpcs/{id}/subnets
POST   /network/vpcs/{id}/subnets      { name, cidr }
DELETE /network/vpcs/{id}/subnets/{subnet}
```

`POST /vpcs/{id}/subnets` is the short road: it makes a network and its one
subnet together, named alike, and joins them to the VPC. `cidr` may be a
prefix length alone (`/24`), in which case the next free block of that size
is taken from the VPC. The long road still works: make a network with
`vpc` set, then a subnet on it.

`GET /networks` and `GET /subnets` take `?vpc=`.

## What stays as it is

- Networks with no `vpc` keep working exactly as now, with routers made by
  hand. Nothing is moved for anyone.
- Security groups stay per project. A rule that names a group still matches
  that group's ports wherever they are.
- Floating IPs, DHCP and the metadata service do not change.
- Workloads pick networks by name as they do now. A network in a VPC is
  still a network.

## Limits

A project gets a `vpcs` limit, 5 unless an admin sets another. The router a
VPC owns is not counted against the project's router limit. Networks and
subnets in a VPC count as they do today.

## Backends

- OVN: the owned router is a logical router, as any router is now. No new
  backend calls.
- overlay: the same, with the limit that already holds there: a router lives
  on one host.
- hostnat: one host, no routing between networks. A VPC on hostnat may hold
  one subnet; a second is refused with a line that says why.
- mock: as OVN.

## Command line

```
pi vpc list | show NAME | create NAME --cidr CIDR [--cidr CIDR] [--external NET] | delete NAME
pi vpc subnet list VPC | add VPC NAME --cidr CIDR|/LEN | remove VPC NAME
pi machine create NAME --vpc VPC --subnet NAME ...
```

## Order of work

1. The resource, its owned router, the limit. Tests.
2. Network membership, the subnet rules, the short road. Tests.
3. `pi vpc`, the wiki page, and a console page. These three do not touch each
   other and can be written at once.
4. Adopt: turn an existing router and the networks on it into a VPC when
   their addresses fit.
5. Peering between two VPCs, in one project first, then across projects with
   an accept step.

## Not in this design

- A VPC that spans clusters. The VPC is the thing such a link would join, and
  the id is stable for that reason, but the link is its own design.
- Placing a load balancer address inside a VPC. Wanted, separate work.
- Names that answer with several healthy addresses. Wanted, separate work.
- A default VPC made for every new project. Open: it makes the first machine
  easier and it takes an address block without being asked.
