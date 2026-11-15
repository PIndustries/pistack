  // ---- what an admin can do ----
  const held = reportHold.get(String(host.id)) || null;
  const drawReport = () => {
    if (!held) return;
    const box = el("div", null, "report");
    box.append(el("h3", held.title));
    if (held.rows === null) {
      box.append(el("p", held.empty || "", "sub"));
    } else {
      const panel = el("div", null, "panel");
      panel.append(simpleTable([
        ["Machine", r => r.name],
        ["Result", r => r.why],
      ], held.rows));
      box.append(panel);
      if (!held.rows.length) box.append(el("p", held.empty, "sub"));
    }
    root.append(box);
    if (box.scrollIntoView) box.scrollIntoView({ block: "nearest" });
  };
  if (!readOnly) {
