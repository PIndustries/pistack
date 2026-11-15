function bindingLine(binding, refresh) {
  const where = bindingScope(binding);
  const row = el("div", null, "quota");
  const what = el("span", (binding.spec && binding.spec.role) || "—", "what");
  const scope = el("span", "held in " + where, "bar");
  row.append(what, document.createTextNode(" "), scope);
  if (refresh) {
    const off = el("button", "Remove"); off.type = "button"; off.className = "danger";
    off.addEventListener("click", async () => {
      if (!confirm("Take " + (binding.spec && binding.spec.role) + " in " + where + " off " + bindingHolder(binding) + "?")) return;
      setErr("");
      off.disabled = true;
      try { await api("DELETE", "/identity/bindings/" + encodeURIComponent(binding.id)); }
      catch (e) { setErr(e.message); off.disabled = false; return; }
      await refresh();
    });
    row.append(off);
  }
  return row;
}
// One user's roles, and what may be done about them. Drawn into the slot
// under the open line, so a change redraws in place.
