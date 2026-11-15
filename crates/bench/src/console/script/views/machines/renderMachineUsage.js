  // ---- usage ----
  const mine = (l) => l.machine === machine.id;
  const rows = samples || [];
  const hist = {
    cpu: sumSeries(seriesOf(rows, "pistack_machine_cpu_usage_cores", mine)),
    mem: sumSeries(seriesOf(rows, "pistack_machine_memory_bytes", mine)),
    memMax: lastOf(sumSeries(seriesOf(rows, "pistack_machine_memory_assigned_bytes", mine))),
    dr: sumSeries(seriesOf(rows, "pistack_machine_disk_read_bytes_total", mine).map(asRate)),
    dw: sumSeries(seriesOf(rows, "pistack_machine_disk_written_bytes_total", mine).map(asRate)),
    rx: sumSeries(seriesOf(rows, "pistack_machine_network_receive_bytes_total", mine).map(asRate)),
    tx: sumSeries(seriesOf(rows, "pistack_machine_network_transmit_bytes_total", mine).map(asRate)),
  };
  const cards = {
    cpu: chartCard("CPU"),
    mem: chartCard("Memory"),
    disk: chartCard("Disk"),
    net: chartCard("Network"),
  };
  const charts = el("div", null, "charts");
  charts.append(cards.cpu.node, cards.mem.node, cards.disk.node, cards.net.node);
  root.append(charts);
  const none = samples === null
    ? "Usage could not be read"
    : power === "on" ? "No usage reported for this machine yet" : "The machine is off";
  const drawCharts = () => {
    const cpu = lastOf(hist.cpu);
    cards.cpu.draw(cpu === null ? null : fmtNum(cpu), cpu === null ? "" : "cores", [hist.cpu], { empty: none });
    const mem = lastOf(hist.mem);
    cards.mem.draw(mem === null ? null : fmtBytes(mem), hist.memMax ? "of " + fmtBytes(hist.memMax) : "",
      [hist.mem], { ceiling: hist.memMax || 0, empty: none });
    const dr = lastOf(hist.dr), dw = lastOf(hist.dw);
    cards.disk.draw(dw === null ? null : fmtBytes(dw) + "/s", dw === null ? "" : "written", [hist.dw, hist.dr],
      { empty: none, legend: dw === null ? [] : ["write", "read " + fmtBytes(dr || 0) + "/s"] });
    const rx = lastOf(hist.rx), tx = lastOf(hist.tx);
    cards.net.draw(rx === null ? null : fmtBytes(rx) + "/s", rx === null ? "" : "in", [hist.rx, hist.tx],
      { empty: none, legend: rx === null ? [] : ["in", "out " + fmtBytes(tx || 0) + "/s"] });
  };
  drawCharts();

