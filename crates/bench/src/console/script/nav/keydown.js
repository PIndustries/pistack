document.addEventListener("keydown", (ev) => {
  if (ev.key !== "/" || ev.metaKey || ev.ctrlKey || ev.altKey) return;
  const t = ev.target;
  if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.tagName === "SELECT")) return;
  if (document.getElementById("shell").hidden) return;
  ev.preventDefault();
  navFilter.focus();
});

