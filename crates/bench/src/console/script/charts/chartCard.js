function chartCard(label) {
  const node = el("div", null, "chart");
  const now = el("div", "", "now");
  const plot = el("div");
  const foot = el("div", "", "foot");
  node.append(el("div", label, "label"), now, plot, foot);
  return {
    node,
    draw(value, unit, lines, opts) {
      opts = opts || {};
      now.replaceChildren(value === null || value === undefined ? "—" : String(value));
      if (unit) { now.append(" "); now.append(el("small", unit)); }
      const svg = sparkline(lines, opts.ceiling);
      if (svg) { svg.setAttribute("aria-label", label + ", last minutes"); plot.replaceChildren(svg); }
      else plot.replaceChildren(el("div", opts.empty || "No samples yet", "none"));
      foot.replaceChildren();
      for (const [i, text] of (opts.legend || []).entries()) foot.append(el("i", text, i ? "b" : ""));
      if (opts.note) foot.append(el("span", opts.note));
    },
  };
}
function pill(text) {
  const t = String(text || "unknown").toLowerCase();
  return el("span", t, "pill " + t.replace(/[^a-z]/g, ""));
}
function kvLine(k, v, mono) {
  const p = el("p");
  p.append(el("span", k, "k"));
  const val = el("span", null, mono ? "v mono" : "v");
  if (v instanceof Node) val.append(v); else val.textContent = show(v);
  p.append(val);
  return p;
}

