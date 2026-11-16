  const hosts = got.hosts;
  const ready = hosts.filter(h => phaseOf(h) === "ready");
  let cpuAll = 0, cpuFree = 0, memAll = 0, memFree = 0;
  for (const h of ready) {
    const cap = (h.spec && h.spec.capacity) || {};
    const free = (h.status && h.status.free) || {};
    cpuAll += cap.vcpus || 0; cpuFree += free.vcpus || 0;
    memAll += cap.memory_mib || 0; memFree += free.memory_mib || 0;
  }
  const machines = got.machines;
  const on = machines.filter(m => phaseOf(m) === "ready" && String(m.status.power || "").toLowerCase() === "on");
  const firing = got.alarms.filter(a => phaseOf(a) === "firing" || String((a.status && a.status.state) || "").toLowerCase() === "firing");
  const volGib = got.volumes.reduce((n, v) => n + ((v.spec && v.spec.size_gib) || 0), 0);

