# PiStack

PiStack is a private cloud written in Rust. It runs virtual machines, containers and bare-metal machines from one binary, one state store and one API. The `pistack` binary is both the control plane (`pistack server`) and the host agent (`pistack agent`). The `pi` command-line client talks to the API.

It is for people who run their own hardware and want what OpenStack gives them (compute, networking, block and object storage, images, identity, DNS, load balancers) without operating a dozen services, a message queue and a database to get it.

## Status

PiStack is early. Version 0.1.0, no releases yet, built from source.

A single node works. You can start a server, log in, and create projects, users, networks, volumes, images and machines through `pi` and the API. In `--dev` mode a mock host stands in for a hypervisor, so the whole API can be tried on a laptop. A three-server control plane forms, replicates, elects a new leader when one stops, and takes the old leader back.

Running real workloads across several hosts is still being built. Images and local volumes are not copied to hosts, an agent cannot yet complete a certificate-authenticated join, OVN is not set up on agents, and there are no packages or container images. A write is committed on the leader before any other server has it. Read [Known limits](Known-limits) before you plan anything around PiStack.

## Pages

- [Quick start](Quick-start): one node in dev mode, first machine with `pi`.
- [Install](Install): build from source.
- [Concepts](Concepts): resources, projects, roles, modules.
- [Running a cluster](Running-a-cluster): servers, leader, failover, backup.
- [Agents and hosts](Agents-and-hosts): joining a host, runtimes.
- [Networking](Networking): the fabric backends and what each needs.
- [Storage](Storage): volumes, images, object storage, shares.
- [Identity and access](Identity-and-access): principals, tokens, keys.
- [CLI reference](CLI-reference): every `pi` command.
- [Configuration reference](Configuration-reference): flags and environment variables.
- [Known limits](Known-limits): what does not work yet.

The source is at [PIndustries/pistack](https://github.com/PIndustries/pistack). Apache-2.0.
