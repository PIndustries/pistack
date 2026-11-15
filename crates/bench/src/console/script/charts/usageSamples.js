async function usageSamples() {
  if (Date.now() - sampleCache.at < 15000) return sampleCache.rows;
  const raw = await api("GET", "/gauge/samples");
  const rows = (Array.isArray(raw) ? raw : itemsOf(raw)).map((s) => {
    const k = parseSeries(s.metric);
    return { key: s.metric, name: k.name, labels: k.labels, t: Date.parse(s.timestamp), v: Number(s.value) };
  }).filter(r => r.t);
  sampleCache = { at: Date.now(), rows };
  return rows;
}
// Points of every series called `name` that `keep` accepts, by series.
