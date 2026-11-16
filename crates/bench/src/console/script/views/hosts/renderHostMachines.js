  // ---- machines here ----
  root.append(el("h3", "Machines on this host (" + here.length + ")"));
  const panel = el("div", null, "panel");
  const table = simpleTable([
    ["Name", m => m.name],
    ["Phase", m => m.status && m.status.phase],
    ["Power", m => m.status && m.status.power],
    ["Size", m => m.spec && m.spec.size],
    ["Address", m => ((m.status && m.status.addresses) || []).join(", ")],
  ], here);
  [...table.querySelectorAll("tbody tr")].forEach((tr, idx) => {
    const m = here[idx];
    if (!m) return;
    const open = () => go("machines", false, m.id);
    nameLink(tr, 0, m.name, open);
    tr.style.cursor = "pointer";
    tr.addEventListener("click", open);
    tr.querySelectorAll("button").forEach((b) => b.addEventListener("click", (ev) => ev.stopPropagation()));
  });
  panel.append(table);
  root.append(panel);

