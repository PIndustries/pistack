function tile(view, label, value, unit, hint, bar, bad) {
  const t = el("button", null, "tile");
  t.type = "button";
  t.append(el("div", label, "label"));
  const v = el("div", String(value), "value");
  if (unit) { v.append(" "); v.append(el("small", unit)); }
  t.append(v);
  if (bar) t.append(bar);
  if (hint) t.append(el("div", hint, bad ? "hint bad" : "hint"));
  t.addEventListener("click", () => go(view));
  return t;
}
function gib(mib) { return Math.round(mib / 102.4) / 10; }

