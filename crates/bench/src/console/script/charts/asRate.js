function asRate(pts) {
  const out = [];
  for (let i = 1; i < pts.length; i++) {
    const dt = (pts[i].t - pts[i - 1].t) / 1000;
    const dv = pts[i].v - pts[i - 1].v;
    if (dt > 0 && dv >= 0) out.push({ t: pts[i].t, v: dv / dt });
  }
  return out;
}
// Several series added up, in steps of fifteen seconds.
