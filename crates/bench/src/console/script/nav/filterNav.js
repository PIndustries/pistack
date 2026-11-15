function filterNav() {
  const q = navFilter.value.trim().toLowerCase();
  const stored = storedGroups();
  for (const g of document.querySelectorAll("nav .group")) {
    let any = false;
    for (const b of g.querySelectorAll("button[data-view]")) {
      const hit = !q || b.firstChild.textContent.toLowerCase().includes(q) || g.dataset.group.toLowerCase().includes(q);
      b.hidden = !hit;
      any = any || hit;
    }
    g.hidden = !any;
    setGroup(g, q ? any : (g.dataset.group in stored ? stored[g.dataset.group] : g.dataset.group === "Compute"));
  }
  const cur = navButton(current);
  if (!q && cur && cur.closest(".group")) setGroup(cur.closest(".group"), true);
}
