  // Anything that is not healthy, across what was fetched.
  const trouble = [];
  for (const v of wanted) {
    for (const i of got[v]) {
      const ph = phaseOf(i);
      if (["error", "failed", "firing"].includes(ph)) trouble.push({ view: v, item: i });
    }
  }
  const split = el("div", null, "split");
  const left = el("div");
  left.append(el("h3", "Needs attention"));
  const panel = el("div", null, "panel");
  if (!trouble.length) {
    panel.append(el("p", "Nothing is in an error state."));
  } else {
    const t = simpleTable([
      ["Kind", r => VIEWS[r.view].title],
      ["Name", r => r.item.name],
      ["Phase", r => r.item.status && r.item.status.phase],
      ["Message", r => shortIds((r.item.status && r.item.status.message) || "").slice(0, 160)],
    ], trouble.slice(0, 12));
    t.className = "wrap";
    [...t.querySelectorAll("tbody tr")].forEach((tr, idx) => {
      const r = trouble[idx];
      const cell = tr.children[1];
      const b = el("button", r.item.name || "—", "linkish"); b.type = "button";
      b.addEventListener("click", () => {
        go(r.view);
        if (r.view === "machines") { openMachine = r.item.id; render(); }
      });
      cell.replaceChildren(b);
    });
    panel.append(t);
  }
  left.append(panel);
