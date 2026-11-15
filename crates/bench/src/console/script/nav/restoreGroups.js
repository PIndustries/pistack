(function restoreGroups() {
  const all = storedGroups();
  for (const g of document.querySelectorAll("nav .group")) {
    const open = g.dataset.group in all ? all[g.dataset.group] : g.dataset.group === "Compute";
    if (!open) setGroup(g, false);
  }
})();
