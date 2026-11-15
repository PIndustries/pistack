function renderLines(items, roles, projects, gen) {
  const onProjects = current === "projects";
  const path = VIEWS[current].path;
  let open = onProjects ? openProject : openUser;
  // What the page loses by not being a table any more: the enabled or
  // disabled word sits in the panel now, and Delete sits beside the name,
  // with the same confirm every other page gives it.
  const controls = (item) => {
    const td = el("span", null, "line-actions");
    if (readOnly) return td;
    const del = el("button", "Delete"); del.type = "button"; del.className = "danger";
    del.addEventListener("click", async (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      if (!confirm("Delete " + (item.name || item.id) + "? This cannot be undone.")) return;
      setErr("");
      del.disabled = true;
      try { await api("DELETE", path + "/" + item.id); await render(); }
      catch (e) { setErr(e.message); del.disabled = false; }
    });
    td.append(del);
    return td;
  };
  return detailLines(
    items,
    (item, name) => { name.textContent = nameOf(item); },
    (item) => {
      const bits = [onProjects ? "limits" : "roles"];
      if (onProjects) {
        bits.unshift(item.spec && item.spec.parent ? "under " + projectName(item.spec.parent) : "top level");
      } else {
        if (item.spec && item.spec.kind) bits.unshift(String(item.spec.kind));
        if (item.spec && item.spec.default_project) bits.push("defaults to " + projectName(item.spec.default_project));
      }
      return bits.filter(Boolean).join(" · ");
    },
    open,
    (item) => String(item.id),
    (id) => { open = id; if (onProjects) openProject = id; else openUser = id; },
    (item, put, gen) => {
      // What the two pages share, above whatever panel they draw: the id in
      // the mono style, and enabled or disabled — the table's own columns.
      const who = el("div", null, "panel who-lines");
      const live = !(item.spec && item.spec.enabled === false);
      const idLine = el("p", "ID ");
      idLine.append(el("span", String(item.id), "mono"));
      who.append(idLine);
      who.append(el("p", "Status " + (live ? "enabled" : "disabled"), live ? "enabled" : "disabled"));
      if (onProjects) {
        return quotaBlock(String(item.id), false).then((node) => {
          if (!node) return;
          put(who, node);
        });
      }
      return userPanel(nameOf(item), roles, projects, gen, put, who);
    },
    controls
  );
}

