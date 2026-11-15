function seriesOf(rows, name, keep) {
  const by = new Map();
  for (const r of rows) {
    if (r.name !== name || (keep && !keep(r.labels))) continue;
    if (!by.has(r.key)) by.set(r.key, []);
    by.get(r.key).push({ t: r.t, v: r.v });
  }
  for (const pts of by.values()) pts.sort((a, b) => a.t - b.t);
  return [...by.values()];
}
// A counter's points as a rate per second. A counter that went down was
// reset, and that step is left out.
