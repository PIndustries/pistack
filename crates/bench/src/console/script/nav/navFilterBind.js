navFilter.addEventListener("input", filterNav);
navFilter.addEventListener("keydown", (ev) => {
  if (ev.key === "Escape") { navFilter.value = ""; filterNav(); navFilter.blur(); return; }
  if (ev.key !== "Enter") return;
  const first = [...document.querySelectorAll("nav .group button[data-view]")].find(b => !b.hidden);
  if (!first || !navFilter.value.trim()) return;
  navFilter.value = "";
  filterNav();
  go(first.dataset.view);
});
