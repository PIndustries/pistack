  if (view.note) {
    const note = el("p", view.note);
    note.id = "note";
    root.append(note);
  }
  if (current === "keys" && keyOnce) {
    const shown = keyOnce;
    keyOnce = null;
    root.append(el("p", "Copy this key secret now. It is not shown again. " + shown, "once"));
  }
  if (form) root.append(form);
  if (view.multi === "dns") { await renderDns(root, gen); return; }
  if (view.multi === "balancers") { await renderBalancers(root, gen); return; }
