  timer = setInterval(async () => {
    if (document.hidden) return;
    let report;
    try { report = await api("GET", "/compute/machines/" + mid + "/stats"); } catch (e) { return; }
    if (gen !== renderGen) return;
    const series = (report && report.series) || [];
    const add = (name, field) => {
      const hits = series.filter(s => s.metric === name && s[field] !== undefined && s[field] !== null);
      if (!hits.length) return [0, null];
      return [Math.max(...hits.map(s => Date.parse(s.timestamp) || 0)), hits.reduce((n, s) => n + Number(s[field]), 0)];
    };
    const feed = (list, name, field) => { const [t, v] = add(name, field); push(list, t, v); };
    feed(hist.cpu, "pistack_machine_cpu_usage_cores", "value");
    feed(hist.mem, "pistack_machine_memory_bytes", "value");
    feed(hist.dr, "pistack_machine_disk_read_bytes_total", "rate");
    feed(hist.dw, "pistack_machine_disk_written_bytes_total", "rate");
    feed(hist.rx, "pistack_machine_network_receive_bytes_total", "rate");
    feed(hist.tx, "pistack_machine_network_transmit_bytes_total", "rate");
    drawCharts();
  }, 10000);
  // A soft pass built the whole page into a detached element. Now that every
  // read has answered and this build is still the newest, move its children
  // into the live view in a single swap and put the scroll back where it was.
  if (soft) {
    if (gen !== renderGen) return;
    const y = window.scrollY;
    live.replaceChildren(...root.childNodes);
    window.scrollTo(0, y);
  }
}

