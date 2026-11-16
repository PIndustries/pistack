function sparkline(lines, ceiling) {
  const W = 300, H = 64;
  const all = lines.flat();
  if (all.length < 2) return null;
  const t0 = Math.min(...all.map(p => p.t));
  const t1 = Math.max(...all.map(p => p.t));
  const top = Math.max(ceiling || 0, ...all.map(p => p.v)) || 1;
  const x = t => (t1 === t0 ? W : (t - t0) / (t1 - t0) * W);
  const y = v => H - 1 - (v / top) * (H - 5);
  const svg = svgEl("svg", { viewBox: "0 0 " + W + " " + H, preserveAspectRatio: "none", role: "img" });
  svg.append(svgEl("line", { class: "base", x1: 0, x2: W, y1: H - 0.5, y2: H - 0.5 }));
  lines.forEach((pts, idx) => {
    if (pts.length < 2) return;
    const d = pts.map((p, i) => (i ? "L" : "M") + x(p.t).toFixed(1) + " " + y(p.v).toFixed(1)).join(" ");
    if (idx === 0) {
      svg.append(svgEl("path", { class: "area", d: d + " L" + x(pts[pts.length - 1].t).toFixed(1) + " " + H + " L" + x(pts[0].t).toFixed(1) + " " + H + " Z" }));
    }
    svg.append(svgEl("path", { class: idx ? "ln b" : "ln", d }));
  });
  return svg;
}
// A card with a name, the value now and the line behind it. `draw` is
// called again whenever there are new points.
