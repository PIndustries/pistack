function sumSeries(list) {
  const STEP = 15000;
  const buckets = new Map();
  list.forEach((pts, idx) => {
    for (const p of pts) {
      const b = Math.round(p.t / STEP) * STEP;
      if (!buckets.has(b)) buckets.set(b, new Map());
      buckets.get(b).set(idx, p.v);
    }
  });
  return [...buckets.entries()].sort((a, b) => a[0] - b[0])
    .map(([t, vals]) => ({ t, v: [...vals.values()].reduce((n, v) => n + v, 0) }));
}
