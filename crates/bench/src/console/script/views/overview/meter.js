function meter(used, total) {
  const m = el("div", null, "meter");
  const pct = total > 0 ? Math.min(100, Math.round(used * 100 / total)) : 0;
  if (pct >= 95) m.classList.add("full"); else if (pct >= 80) m.classList.add("high");
  const bar = el("i");
  bar.style.width = pct + "%";
  m.append(bar);
  m.title = pct + "% in use";
  return m;
}
