  // ---- usage ----
  const node = spec.node_name || "";
  const mine = (l) => l.host === node && !l.machine;
  const rows = samples || [];
  const hist = {
    load: sumSeries(seriesOf(rows, "pistack_host_load1", mine)),
    memUsed: sumSeries(seriesOf(rows, "pistack_host_memory_used_bytes", mine)),
    memTotal: sumSeries(seriesOf(rows, "pistack_host_memory_total_bytes", mine)),
    diskFree: sumSeries(seriesOf(rows, "pistack_host_data_disk_free_bytes", mine)),
    diskTotal: sumSeries(seriesOf(rows, "pistack_host_data_disk_total_bytes", mine)),
    machines: sumSeries(seriesOf(rows, "pistack_host_machines", mine)),
  };
  // The ring keeps one point per sample; the page adds points at the same
  // fifteen second step, so the line is drawn on one grid either way.
  for (const k of Object.keys(hist)) hist[k] = hist[k].map(p => ({ t: Math.round(p.t / 15000) * 15000, v: p.v }));
  const cards = {
    load: chartCard("Load"),
    mem: chartCard("Memory"),
    disk: chartCard("Disk free"),
    placed: chartCard("Machines"),
  };
  const charts = el("div", null, "charts");
  charts.append(cards.load.node, cards.mem.node, cards.disk.node, cards.placed.node);
  root.append(charts);
  const noData = samples === null ? "Usage could not be read" : "No usage reported for this host yet";
  const drawCharts = () => {
    const l = lastOf(hist.load);
    cards.load.draw(l === null ? null : fmtNum(l), l === null ? "" : "over one minute", [hist.load], { empty: noData });
    const used = lastOf(hist.memUsed), total = lastOf(hist.memTotal);
    cards.mem.draw(used === null ? null : fmtBytes(used), total ? "of " + fmtBytes(total) : "", [hist.memUsed],
      { ceiling: total || 0, empty: noData, note: total ? Math.round((used || 0) * 100 / total) + "% in use" : "" });
    const free = lastOf(hist.diskFree), diskAll = lastOf(hist.diskTotal);
    cards.disk.draw(free === null ? null : fmtBytes(free), diskAll ? "of " + fmtBytes(diskAll) : "", [hist.diskFree],
      { ceiling: diskAll || 0, empty: noData, note: diskAll ? Math.round((diskAll - (free || 0)) * 100 / diskAll) + "% full" : "" });
    const n = lastOf(hist.machines);
    cards.placed.draw(n === null ? null : String(Math.round(n)), n === null ? "" : "as last reported", [hist.machines], { empty: noData });
  };
  drawCharts();

