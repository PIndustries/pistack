async function quotaBlock(project, cappedOnly) {
  let report = null;
  try {
    report = await api("GET", "/identity/projects/" + encodeURIComponent(project) + "/quotas");
  } catch (e) {
    if (cappedOnly) return null;
    const box = el("div", null, "panel");
    box.append(el("p", "The limits could not be read: " + e.message));
    return box;
  }
  let named = quotaNames(report);
  if (cappedOnly) named = named.filter(r => report.quotas[r].limit != null);
  // Each resource keeps its own copy, so the answer to a write can replace
  // one line without taking the others with it.
  const lines = {};
  for (const r of named) lines[r] = { limit: report.quotas[r].limit, used: report.quotas[r].used };
  const out = el("div");
  const show = () => {
    const box = el("div", null, "panel");
    for (const r of named) box.append(quotaLine(r, lines[r]));
    if (cappedOnly) { out.replaceChildren(box); return; }
    // The panel is one node end to end: the limits and the Edit that owns
    // them, so the slot takes the whole of it in one go. A limit is set by
    // an admin alone — the same rule every other write on the page follows.
    if (!readOnly) {
      const tools = el("div", null, "tools");
      const edit = el("button", "Edit"); edit.type = "button";
      edit.addEventListener("click", () => out.replaceChildren(quotaEditor(String(project), named, lines, show)));
      tools.append(edit);
      const wrap = el("div");
      wrap.append(box, tools);
      out.replaceChildren(wrap);
      return;
    }
    out.replaceChildren(box);
  };
  show();
  return out;
}

