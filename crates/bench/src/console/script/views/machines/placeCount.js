function placeCount(machines) {
  const out = new Map();
  for (const m of machines) {
    const h = machineHost(m);
    if (h) out.set(h, (out.get(h) || 0) + 1);
  }
  return out;
}
