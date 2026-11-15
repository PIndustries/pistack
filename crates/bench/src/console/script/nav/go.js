function go(view, fromHash, id) {
  if (!VIEWS[view]) view = "overview";
  current = view;
  // The host page keeps its own address, so reload and Back land back on
  // the host. A machine is opened from the list, which stays mounted under
  // it, so its address is left alone: Back returns to the list.
  const openable = id && view === "hosts" ? id : null;
  openHost = view === "hosts" ? openable : null;
  openMachine = view === "machines" ? (id || null) : null;
  openProject = null;
  openUser = null;
  for (const n of document.querySelectorAll("nav button[data-view]")) n.removeAttribute("aria-current");
  const b = navButton(view);
  let group = "";
  if (b) {
    b.setAttribute("aria-current", "page");
    const g = b.closest(".group");
    if (g) { group = g.dataset.group; setGroup(g, true); }
  }
  document.getElementById("crumb-group").textContent = group;
  document.getElementById("crumb-title").textContent = VIEWS[view].title;
  document.title = VIEWS[view].title + " · PiStack";
  document.getElementById("filter").value = "";
  document.getElementById("shell").classList.remove("nav-open");
  // The machine page keeps the list's address: a reload or a Back from
  // there is meant to land back on the list. The host page keeps its own
  // address, so reloading it returns to the host.
  const want = "#/" + view + (id && view === "hosts" ? "/" + encodeURIComponent(id) : "");
  if (location.hash !== want) history.pushState(null, "", want);
  render();
}
