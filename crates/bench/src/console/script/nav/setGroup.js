function setGroup(g, open, remember) {
  g.querySelector(".group-head").setAttribute("aria-expanded", String(open));
  g.querySelector(".group-items").hidden = !open;
  if (!remember) return;
  const all = storedGroups();
  all[g.dataset.group] = open;
  try { localStorage.setItem("pistack.nav", JSON.stringify(all)); } catch (e) { /* private window */ }
}
