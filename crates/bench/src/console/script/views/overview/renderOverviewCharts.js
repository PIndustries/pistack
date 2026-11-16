  if (samples !== null) {
    const rows = samples;
    const load = sumSeries(seriesOf(rows, "pistack_host_load1"));
    const memUsed = sumSeries(seriesOf(rows, "pistack_host_memory_used_bytes"));
    const memTotal = lastOf(sumSeries(seriesOf(rows, "pistack_host_memory_total_bytes")));
    const cores = sumSeries(seriesOf(rows, "pistack_machine_cpu_usage_cores"));
    const rx = sumSeries(seriesOf(rows, "pistack_machine_network_receive_bytes_total").map(asRate));
    const tx = sumSeries(seriesOf(rows, "pistack_machine_network_transmit_bytes_total").map(asRate));
    const empty = "No samples on this server. The leader keeps them.";
    const charts = el("div", null, "charts");
    const c1 = chartCard("Host load"), c2 = chartCard("Host memory"), c3 = chartCard("Machine CPU"), c4 = chartCard("Machine network");
    charts.append(c1.node, c2.node, c3.node, c4.node);
    const v1 = lastOf(load), v2 = lastOf(memUsed), v3 = lastOf(cores), v4 = lastOf(rx);
    c1.draw(v1 === null ? null : fmtNum(v1), v1 === null ? "" : "across " + ready.length + (ready.length === 1 ? " host" : " hosts"), [load], { empty });
    c2.draw(v2 === null ? null : fmtBytes(v2), memTotal ? "of " + fmtBytes(memTotal) : "", [memUsed], { ceiling: memTotal || 0, empty });
    c3.draw(v3 === null ? null : fmtNum(v3), v3 === null ? "" : "cores busy", [cores], { empty });
    c4.draw(v4 === null ? null : fmtBytes(v4) + "/s", v4 === null ? "" : "in", [rx, tx],
      { empty, legend: v4 === null ? [] : ["in", "out " + fmtBytes(lastOf(tx) || 0) + "/s"] });
    root.append(charts);
  }

