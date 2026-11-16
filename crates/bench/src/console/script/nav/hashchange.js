window.addEventListener("hashchange", () => {
  if (!token() || document.getElementById("shell").hidden) return;
  const at = hashView();
  if (at.view === current && (at.view !== "hosts" || at.id === openHost)
      && (at.view !== "machines" || !at.id || at.id === openMachine)) return;
  go(at.view, true, at.id);
});

