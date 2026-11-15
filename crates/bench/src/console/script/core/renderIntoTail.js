  if (current === "volumes") {
    try {
      const snaps = itemsOf(await api("GET", "/volume/snapshots"));
      if (gen !== renderGen) return;
      root.append(el("h3", "Snapshots"));
      root.append(simpleTable([
        ["Name", i => i.name],
        ["Phase", i => i.status && i.status.phase],
        ["Volume", i => i.spec && i.spec.volume],
        ["GiB", i => i.status && i.status.size_gib],
      ], snaps));
    } catch (e) {
      if (gen !== renderGen) return;
      setErr(e.message);
    }
  }
}

