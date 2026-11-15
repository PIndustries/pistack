function line(k, v, phase) {
  const p = el("p");
  p.append(el("span", k, "k"));
  const val = el("span", show(v));
  if (phase) val.className = "phase " + String(v || "");
  p.append(val);
  return p;
}

// ---- usage charts, times and sizes ----------------------------------------

