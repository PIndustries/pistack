function quotaEditor(project, named, lines, done) {
  const panel = el("div", null, "panel");
  const held = {};
  for (const r of named) {
    const row = el("div", null, "quota edit");
    const input = el("input");
    input.type = "number";
    input.min = "0";
    input.step = "1";
    input.placeholder = "no limit";
    input.setAttribute("aria-label", quotaInfo(r)[1] + " limit");
    const holder = el("span", null, "bar");
    holder.append(input);
    row.append(el("span", quotaInfo(r)[1], "what"), holder);
    const figure = el("span", null, "used");
    row.append(figure);
    panel.append(row);
    held[r] = { input, figure, was: "" };
  }
  for (const r of named) {
    const limit = lines[r].limit;
    const used = Number(lines[r].used) || 0;
    held[r].was = limit == null ? "" : String(limit);
    held[r].input.value = held[r].was;
    held[r].figure.textContent = limit == null
      ? quotaValue(r, used) + " used"
      : quotaValue(r, used) + " of " + quotaValue(r, limit);
    held[r].figure.className = limit != null && used >= limit ? "used bad" : "used";
  }
  const tools = el("div", null, "tools");
  const save = el("button", "Save limits"); save.type = "button"; save.className = "primary";
  const cancel = el("button", "Cancel"); cancel.type = "button";
  save.addEventListener("click", async () => {
    const quotas = {};
    setErr("");
    try {
      for (const r of named) {
        const now = held[r].input.value.trim();
        if (now === held[r].was) continue;
        if (now === "") quotas[r] = null;
        else quotas[r] = intField(now, quotaInfo(r)[1], 0);
      }
    } catch (e) { setErr(e.message); return; }
    if (!Object.keys(quotas).length) { done(); return; }
    save.disabled = true;
    try {
      const fresh = await api("PUT", "/identity/projects/" + encodeURIComponent(project) + "/quotas", { quotas });
      for (const r of Object.keys((fresh && fresh.quotas) || {})) {
        if (lines[r]) { lines[r].limit = fresh.quotas[r].limit; lines[r].used = fresh.quotas[r].used; }
      }
      // The write is through: back to the bars, which now show what the
      // server holds.
      done();
    } catch (e) { setErr(e.message); save.disabled = false; }
  });
  cancel.addEventListener("click", () => done());
  tools.append(save, cancel);
  const wrap = el("div");
  wrap.append(panel, tools);
  return wrap;
}
// One project's limits, with Edit for a caller that may change them.
// `cappedOnly` is the overview's ask: nothing at all comes back when the
// report cannot be read or nothing in it has a limit.
