  const tiles = el("div", null, "tiles");
  tiles.append(
    tile("machines", "Machines", on.length, "of " + machines.length + " running"),
    tile("hosts", "Hosts", ready.length, "of " + hosts.length + " ready",
      hosts.length > ready.length ? (hosts.length - ready.length) + " not ready" : "", null, true),
    tile("hosts", "vCPU in use", cpuAll - cpuFree, "of " + cpuAll, cpuFree + " free", meter(cpuAll - cpuFree, cpuAll)),
    tile("hosts", "Memory in use", gib(memAll - memFree), "of " + gib(memAll) + " GiB", gib(memFree) + " GiB free", meter(memAll - memFree, memAll)),
    tile("pods", "Pods", got.pods.length),
    tile("networks", "Networks", got.networks.length, "", got.fips.length + " floating IPs"),
    tile("volumes", "Volumes", got.volumes.length, "", volGib + " GiB provisioned"),
    tile("alarms", "Alarms firing", firing.length, "of " + got.alarms.length, "", null, firing.length > 0),
  );
  root.append(tiles);

  // What the project you are working in is capped at, when anything caps it.
  if (capped) {
    root.append(el("h3", "Quota"));
    root.append(capped);
  }

