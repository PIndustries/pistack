async function userPanel(name, roles, projects, gen, put, who) {
  const out = el("div");
  if (who) out.append(who);
  const box = el("div", null, "panel");
  let mine = [];
  try {
    const [bindings, principals] = await Promise.all([
      api("GET", "/identity/bindings?limit=1000"),
      api("GET", "/identity/principals?name=" + encodeURIComponent(name)),
    ]);
    if (gen !== renderGen) return out;
    const me = itemsOf(principals).find(p => nameOf(p) === name);
    mine = me ? itemsOf(bindings).filter(b => b.spec && String(b.spec.principal) === String(me.id)) : [];
  } catch (e) {
    if (gen !== renderGen) return out;
    box.append(el("p", "You are not allowed to see which roles are held."));
    out.append(box);
    put(out);
    return out;
  }
  if (!mine.length) box.append(el("p", "No role is held anywhere."));
  // A change here is drawn by a fresh panel, from this same slot, rather
  // than a page-wide redraw: the write itself already left a full page
  // build behind it, and that build cannot see a binding that lands after.
  // The newest build answers, not the one this panel was drawn under: a
  // write from a panel older than the page still shows its result.
  const again = () => userPanel(name, roles, projects, renderGen, put, who);
  for (const b of mine) box.append(bindingLine(b, readOnly ? null : again));
  out.append(box);
  put(out);
  if (readOnly) return out;

  const tools = el("div", null, "tools");
  const form = el("form");
  form.className = "create";
  form.append(el("strong", "Add role"));
  const fields = el("div"); fields.className = "row";
  const role = el("select");
  role.setAttribute("aria-label", "Role for " + name);
  role.append(option("", "Role…"));
  for (const r of roles) role.append(option(r.name, r.name));
  const scopeSel = el("select");
  scopeSel.setAttribute("aria-label", "Where that role applies for " + name);
  scopeSel.append(option("", "Whole cloud"));
  for (const p of projects) scopeSel.append(option(nameOf(p), nameOf(p)));
  fields.append(field("Role", role), field("Where", scopeSel));
  const add = el("button", "Add"); add.type = "submit"; add.className = "primary";
  form.append(fields, add);
  onCreate(form, add, async () => {
    if (!role.value) throw new Error("Pick a role first.");
    const body = { principal: name, role: role.value };
    if (scopeSel.value) body.project = scopeSel.value;
    await api("POST", "/identity/bindings", body);
    await again();
    // The panel answers for the write; no page-wide build follows it.
    return true;
  });
  const sign = el("button", "Sign out everywhere"); sign.type = "button";
  sign.title = "Refuse every token issued to this user up to now";
  sign.addEventListener("click", async () => {
    if (!confirm("Refuse every token " + name + " holds? They have to sign in again.")) return;
    setErr("");
    sign.disabled = true;
    try { await api("POST", "/identity/principals/" + encodeURIComponent(name) + "/revoke-tokens"); }
    catch (e) { setErr(e.message); sign.disabled = false; }
  });
  tools.append(form, el("span", null, "grow"), sign);
  out.append(tools);
  put(out);
  return out;
}

// One line per entry, and the open one grows a panel below itself. The
// panel is only in the document while it is shown: `open` is what was open
// when the page was drawn, `openTo` records a new choice, and `draw` fills
// the slot under the line. `title` writes the line's own heading, so a row
// can carry controls beside its name.
