async function loadProjects() {
  let mine;
  try { mine = await api("GET", "/identity/tokens/self/projects"); }
  catch (e) { scopeBox.hidden = true; return; }
  if (!Array.isArray(mine)) { scopeBox.hidden = true; return; }
  myProjects = mine;
  const here = mine.find(p => p.current);
  if (!mine.length) {
    scopeBox.hidden = true;
    if (!here) setErr("You have no role in any project yet. Ask an admin to add you to one.");
    return;
  }
  // Signed in without a project: go to the one used last, else the first.
  // Only on the way in. This runs in the background of every page, and a
  // switch taken up here later would clear the page open on screen and
  // redraw its list under the hand.
  if (!here && !autoPicked) {
    autoPicked = true;
    const last = lastProject();
    const pick = mine.find(p => p.name === last) || mine[0];
    await switchProject(pick.name);
    return;
  }
  if (!here) return;
  const tenants = [...new Set(mine.map(tenantOf))].sort();
  const tenant = here ? tenantOf(here) : "";
  tenantBox.replaceChildren();
  if (!here) {
    const none = option("", "No tenant");
    none.selected = true;
    none.disabled = true;
    tenantBox.append(none);
  }
  for (const t of tenants) {
    const o = option(t, t);
    o.selected = t === tenant;
    tenantBox.append(o);
  }
  tenantBox.disabled = tenants.length < 2 && !!here;
  projectBox.replaceChildren();
  if (!here) {
    const none = option("", "No project");
    none.selected = true;
    none.disabled = true;
    projectBox.append(none);
  }
  const inTenant = mine.filter(p => tenantOf(p) === tenant);
  for (const p of inTenant) {
    const o = option(p.name, p.name);
    o.selected = !!p.current;
    o.title = (p.roles || []).join(", ");
    projectBox.append(o);
  }
  projectBox.disabled = inTenant.length < 2;
  scopeBox.hidden = false;
}
// Move to another project: a new token for it, which ends when this one
// would have, and everything on screen read again.
