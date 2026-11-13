# Metrics and alarms

PiStack measures its hosts, its machines and its own control plane, and serves the figures in the Prometheus text format. It does not keep history. A server holds the last few minutes in memory, which is enough for `pi machine stats` and for alarms. Everything older is the job of a Prometheus server that scrapes it.

## Where the figures come from

**Hosts and machines.** Each host agent adds a usage report to its heartbeat, at most once every 10 seconds, so with the default 15-second heartbeat every heartbeat carries one. The report has the host's load, memory and data-directory disk, and for each running machine its CPU time, memory, and disk and network counters.

Machine figures come from the runtime:

| Runtime | Usage |
| --- | --- |
| libvirt/KVM | One `virsh domstats` call for all running guests per report. |
| mock | Made-up figures, so the path can be tested without a hypervisor. |
| cloud-hypervisor, youki containers, bare metal | Not measured. These machines have no usage series. |

A report waits at most five seconds for the runtime; if `virsh` does not answer in that time the heartbeat goes out with the host's figures only. The built-in host of `--dev` mode has no agent and reports nothing.

On the wire a machine with one disk and one interface is about 300 bytes of JSON, so a host with 100 machines adds some 30 KiB to a heartbeat.

**The control plane.** Every server records its own state every 15 seconds, and again when it is scraped: its role, term and log position, how far behind each follower is, the size of its store, how many reconciles its modules failed, and how many commands it holds for each agent.

## What a server keeps

- The last 60 points of each series, and nothing older than an hour.
- At most 50,000 series. A machine with one disk and one interface is about a dozen, so that is room for some four thousand machines. Past the limit, samples for new series are dropped and the server logs that once.
- In memory only. Nothing is written to disk, and nothing is copied to the other servers.

So each server has its own window. Agents report to the leader, which makes the leader the only server that has host and machine series. A server that restarts, or one that has just become leader, starts empty and fills up as the agents report. A file `gauge/samples.json` left in the data directory by an older build is no longer read and can be deleted.

A series that has had no sample for five minutes is left out of `/metrics`, of `pi machine stats` and of alarms. That is how a deleted machine or a dead host goes away.

## Current values

```sh
pi machine stats web
pi host stats rack1
```

```text
METRIC                               VALUE      RATE
cpu_seconds_total                    5400.0 s   0.50 cpus
cpu_usage_cores                      0.50
memory_bytes                         1.0 GiB
disk_written_bytes_total (vda)       3.0 GiB    2.0 KiB/s
network_receive_bytes_total (vnet2)  1.2 GiB    48.5 KiB/s
```

The rate of a counter is taken from its last two samples. `-o json` prints every series with its labels. Anyone who can see a machine can read its stats; host stats are for a cloud admin. A follower sends both requests to the leader, and `pi` follows.

The API is `GET /compute/machines/{id}/stats` and `GET /compute/hosts/{id}/stats`.

## Prometheus

`GET /metrics` at the root of the API serves everything the server holds. It is not open: send a bearer token that is either

- the scrape token, set on the server with `PISTACK_METRICS_TOKEN` (at least 16 characters). It can read `/metrics` and nothing else; or
- the token of a cloud admin.

In `--dev` mode no token is asked for, as for the rest of the API. `GET /gauge/metrics` serves the same text to a cloud admin's token only.

Scrape every server, because each one answers for itself. The host and machine series come from whichever server leads at the time.

```yaml
scrape_configs:
  - job_name: pistack
    scheme: http            # https when you scrape the --tls-listen port
    metrics_path: /metrics
    scrape_interval: 30s
    authorization:
      type: Bearer
      credentials_file: /etc/prometheus/pistack-scrape-token
    static_configs:
      - targets:
          - 10.0.0.11:8443
          - 10.0.0.12:8443
          - 10.0.0.13:8443
```

The HTTPS listener presents a certificate from the cloud CA. Give Prometheus that CA with `tls_config.ca_file`.

Because host and machine series move with the leader, the `instance` label on them changes after a failover. Aggregate over it in queries, for example `max without (instance) (pistack_machine_memory_bytes)`.

## Metric names

The names and labels below are stable. Counters end in `_total`; take `rate()` of them.

### Hosts

Label: `host`, the node name.

| Metric | Meaning |
| --- | --- |
| `pistack_host_vcpus` | CPUs of the host. |
| `pistack_host_memory_mib` | Memory of the host in MiB, as it registered. |
| `pistack_host_load1`, `_load5`, `_load15` | Load averages. |
| `pistack_host_memory_total_bytes` | Memory of the host. |
| `pistack_host_memory_used_bytes` | Total less what the kernel reports as available. |
| `pistack_host_data_disk_total_bytes` | Size of the filesystem that holds the agent's data directory. |
| `pistack_host_data_disk_free_bytes` | Free space on it. |
| `pistack_host_machines` | Machines the host's runtimes hold, running or not. |

### Machines

Labels: `machine` (the id), `name`, `project` (the id) and `host`. Disk series add `device`, network series add `interface`.

| Metric | Meaning |
| --- | --- |
| `pistack_machine_cpu_seconds_total` | CPU time used. |
| `pistack_machine_cpu_usage_cores` | CPUs kept busy between the host's last two reports. |
| `pistack_machine_memory_bytes` | Host memory the machine holds (resident set). |
| `pistack_machine_memory_assigned_bytes` | Memory the machine is given now (the balloon). |
| `pistack_machine_memory_guest_used_bytes` | Memory in use inside the guest. Only when the guest has a balloon driver that reports. |
| `pistack_machine_disk_read_bytes_total`, `_written_bytes_total` | Bytes read from and written to a disk. |
| `pistack_machine_disk_reads_total`, `_writes_total` | Read and write requests. |
| `pistack_machine_network_receive_bytes_total`, `_transmit_bytes_total` | Bytes the machine received and sent on an interface. |

The `interface` label is the name of the host-side device, such as `vnet2`. It can change when the machine is restarted. The cloud-init seed disk is not reported.

### Control plane

| Metric | Labels | Meaning |
| --- | --- | --- |
| `pistack_build_info` | `build` | Always 1. |
| `pistack_cluster_is_leader` | `server` | 1 on the leader. The sum over all servers should be 1. |
| `pistack_cluster_is_voter` | `server` | 1 on a voter, 0 on a learner. |
| `pistack_cluster_term` | `server` | Election term. It rises with every election. |
| `pistack_cluster_commit_index` | `server` | Last entry in the server's log. |
| `pistack_cluster_members` | `server`, `role` | Servers by role, as this server's store has them. |
| `pistack_cluster_follower_lag_entries` | `server`, `member` | On the leader: entries a member was behind when it last pulled. |
| `pistack_cluster_follower_last_pull_seconds` | `server`, `member` | On the leader: seconds since a member last pulled. |
| `pistack_reconcile_failures_total` | `module` | Reconcile passes and steps that ended in an error since the server started. |
| `pistack_agent_commands_queued` | `host` | Commands waiting for a host's agent to fetch them. |
| `pistack_agent_commands_in_flight` | `host` | Commands an agent has fetched and not answered. |
| `pistack_store_file_bytes` | | Size of the store file. |
| `pistack_store_log_entries` | | Replication log entries the store holds. |
| `pistack_store_table_rows`, `pistack_store_table_bytes` | `table` | Rows and bytes per table, counted every five minutes. |
| `pistack_gauge_series` | | Series the server holds in memory. |

`server` is the id `pi cluster status` shows. A member that has never pulled from the leader has no lag series, so alert on its absence as well as on its value.

Some rules worth having:

```yaml
- alert: PiStackNoLeader
  expr: sum(pistack_cluster_is_leader) != 1
  for: 1m
- alert: PiStackFollowerBehind
  expr: pistack_cluster_follower_lag_entries > 1000 or pistack_cluster_follower_last_pull_seconds > 30
  for: 2m
- alert: PiStackReconcileFailing
  expr: rate(pistack_reconcile_failures_total[10m]) > 0
  for: 10m
- alert: PiStackAgentNotFetching
  expr: pistack_agent_commands_queued > 20
  for: 5m
```

## Your own samples

```sh
pi api POST /gauge/samples -d '{"metric":"queue.depth","value":12}'
pi api GET '/gauge/samples?metric=queue.depth'
```

A posted sample lives in the same window and appears in `/metrics` as a gauge, with characters that are not legal in a metric name turned into `_`. Names that start with `pistack_` cannot be posted. `GET /gauge/samples` returns the whole window to a cloud admin. Anyone else gets what was posted, plus the `pistack_` series of their own project.

## Alarms

An alarm compares a metric with a threshold. It is `Firing` while the value crosses, `Ok` while it does not, and `Pending` until there is a sample.

```sh
pi api POST /gauge/alarms -d '{
  "name": "web-busy",
  "spec": {
    "name": "web-busy",
    "metric": "pistack_machine_cpu_usage_cores{name=\"web\"}",
    "comparison": "gt",
    "threshold": 1.5,
    "for_seconds": 0
  }
}'
pi api GET /gauge/alarms/web-busy
```

`metric` is a name with or without labels. With labels, the alarm watches every series that has those labels; with none, every series of that name. It fires while any of them crosses, and `status.series` names the one that decided. `comparison` is `gt`, `lt` or `eq`.

An alarm that belongs to a project sees only the `pistack_` series of that project, so it can watch the project's machines and nothing else. An alarm made by a cloud admin sees everything, hosts and the control plane included.

Alarms are evaluated when a sample arrives, on the server that received it. The phase is written to the store when it changes; while it stays the same, `last_value` is refreshed at most once a minute. Only the leader can write to the store, so an alarm follows the series the leader has: hosts, machines, posted samples and the leader's own control-plane series. For a follower's control-plane series use Prometheus rules.

What alarms do not do: `for_seconds` is stored and ignored, so an alarm fires on the first sample that crosses. Nothing is sent anywhere when one fires. An alarm on a series that stops reporting keeps its last phase. For paging, use Prometheus rules.
