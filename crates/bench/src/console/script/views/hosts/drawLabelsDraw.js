  const draw = () => {
    const open = draft.open;
    shown.hidden = open;
    editBtn.hidden = open || readOnly;
    lbox.hidden = !open;
    foot.hidden = !open;
    if (!open) return;
    // Built for this spell of editing; a redraw starts from the draft.
    lbox.replaceChildren();
    foot.replaceChildren();
    const table = el("table");
    const head = el("tr");
    head.append(el("th", "Key"), el("th", "Value"), el("th", ""));
    const thead = el("thead");
    thead.append(head);
    table.append(thead, el("tbody"));
    lbox.append(table);
    for (const r of draft.rows) {
      const tr = el("tr");
      const key = el("input"); key.value = r.key; key.placeholder = "env";
      const val = el("input"); val.value = r.value; val.placeholder = "prod";
      key.setAttribute("aria-label", "Label key");
      val.setAttribute("aria-label", "Label value");
      key.addEventListener("input", () => { r.key = key.value; });
      val.addEventListener("input", () => { r.value = val.value; });
      const td1 = el("td"); td1.append(key);
      const td2 = el("td"); td2.append(val);
      const td3 = el("td");
      const x = el("button", "Remove", "danger"); x.type = "button";
      x.title = "Take this label off the host when the set is saved";
      x.setAttribute("aria-label", "Remove label " + (r.key || "new"));
      x.addEventListener("click", () => { draft.rows.splice(draft.rows.indexOf(r), 1); draw(); });
      td3.append(x);
      tr.append(td1, td2, td3);
      table.querySelector("tbody").append(tr);
    }
    if (!draft.rows.length) {
      const tr = el("tr");
      const td = el("td", "Nothing here yet. Add the first label.", "empty");
      td.colSpan = 3;
      tr.append(td);
      table.querySelector("tbody").append(tr);
    }
    const add = el("button", "Add label"); add.type = "button";
    const save = el("button", "Save labels", "primary"); save.type = "button";
    const cancel = el("button", "Cancel"); cancel.type = "button";
    add.addEventListener("click", () => { draft.rows.push({ key: "", value: "" }); draw(); });
    cancel.addEventListener("click", () => {
      draft.open = false;
      draft.rows = owned.map(k => ({ key: k, value: all[k] }));
      draw();
    });
    save.addEventListener("click", async () => {
      const out = {};
      for (const r of draft.rows) out[String(r.key || "").trim()] = String(r.value || "").trim();
      setErr("");
      save.disabled = true;
      try { await api("PUT", "/compute/hosts/" + encodeURIComponent(host.id) + "/labels", { labels: out }); }
      catch (e) { setErr(e.message); save.disabled = false; return; }
      draft.close();
      await render();
    });
    foot.append(add, save, cancel);
  };
