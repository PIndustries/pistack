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
  if (current === "keypairs" && keypairOnce) {
    const shown = keypairOnce;
    keypairOnce = null;
    const box = el("div", null, "once");
    const secret = el("textarea");
    secret.readOnly = true;
    secret.rows = 12;
    secret.value = shown;
    box.append(el("p", "Save this private key now. It is not kept."), secret);
    root.append(box);
  }
  if (form) root.append(form);
  if (view.multi === "dns") { await renderDns(root, gen); return; }
  if (view.multi === "balancers") { await renderBalancers(root, gen); return; }
