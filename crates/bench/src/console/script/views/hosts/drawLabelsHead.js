function drawLabels(box, host) {
  const all = host.labels || {};
  const sys = Object.keys(all).filter(k => k.startsWith(SYS_LABEL)).sort();
  const owned = Object.keys(all).filter(k => !k.startsWith(SYS_LABEL)).sort();
  // A draft belongs to one host and lives until it is saved or cancelled.
  // The edit state and the typed rows live here, not in the nodes, so the
  // page redrawing itself under the hand keeps both.
  if (labelDraft.host !== String(host.id)) labelDraft.reset(host.id, owned.map(k => ({ key: k, value: all[k] })));
  const draft = labelDraft;
  box.replaceChildren();
  const chips = (title, keys, why) => {
    if (!keys.length) return null;
    const list = el("div", null, "strip");
    list.append(el("span", title, "name"));
    for (const k of keys) {
      const tag = el("span", k + "=" + all[k], "srv");
      tag.title = why;
      list.append(tag);
    }
    return list;
  };
  // Read-only first, so the page shows what the host carries before an
  // editor is asked for. The host's own labels above PiStack's.
  const shown = el("div");
  shown.append(chips("Labels", owned, "An operator's label. Edit labels to change the set.")
    || el("p", "No labels of yours on this host. The zone label is what puts a host in an availability zone the launch wizard offers.", "sub"));
  const bySys = chips("Set by PiStack", sys, "PiStack sets this label. It is shown here and not sent back.");
  if (bySys) shown.append(bySys);
  const editBtn = el("button", "Edit labels"); editBtn.type = "button";
  const lbox = el("div", null, "panel");
  const foot = el("p", null, "tools");
