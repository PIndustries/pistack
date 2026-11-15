  // ---- keeping it current ----
  // The machine page's feed for a host: the host's own latest points, read
  // every ten seconds and drawn without rebuilding the page.
  const push = (list, t, v) => {
    if (v === null || (list.length && list[list.length - 1].t >= t)) return;
    list.push({ t, v });
    if (list.length > 120) list.shift();
  };
  timer = setInterval(async () => {
    if (document.hidden) return;
    let rep = null;
    try { rep = await api("GET", "/compute/hosts/" + hid + "/stats"); } catch (e) { return; }
    if (gen !== renderGen) return;
    const series = (rep && rep.series) || [];
    const last = Math.max(...series.map(s => Date.parse(s.timestamp) || 0), 0);
    const bucket = Math.round(last / 15000) * 15000;
    const pick = (name, field) => {
      const hits = series.filter(s => s.metric === name && s[field] !== undefined && s[field] !== null);
      if (!hits.length) return null;
      return hits.reduce((n, s) => n + Number(s[field]), 0);
    };
    push(hist.load, bucket, pick("pistack_host_load1", "value"));
    push(hist.memUsed, bucket, pick("pistack_host_memory_used_bytes", "value"));
    push(hist.memTotal, bucket, pick("pistack_host_memory_total_bytes", "value"));
    push(hist.diskFree, bucket, pick("pistack_host_data_disk_free_bytes", "value"));
    push(hist.diskTotal, bucket, pick("pistack_host_data_disk_total_bytes", "value"));
    push(hist.machines, bucket, pick("pistack_host_machines", "value"));
    drawCharts();
  }, 10000);
}

// Labels an operator owns, in rows sent back as one set. Labels PiStack
// wrote are shown and left out of the set; the server keeps those.
