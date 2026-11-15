async function renderInto(root, view, gen) {
  if (current === "overview") { await renderOverview(root, gen); return; }
  if (current === "topology") { await renderTopology(root, gen); return; }
  const form = readOnly ? null : createForm();
  const tools = el("div"); tools.className = "tools";
  tools.append(el("h2", view.title));
  const chip = el("span", null, "chip");
  tools.append(chip, el("span", null, "grow"));
  if (form) {
    const open = !!formOpen[current];
    form.hidden = !open;
    const toggle = el("button", open ? "Close" : "Create", open ? "" : "primary");
    toggle.type = "button";
    toggle.addEventListener("click", () => {
      formOpen[current] = form.hidden;
      form.hidden = !form.hidden;
      toggle.textContent = form.hidden ? "Create" : "Close";
      toggle.className = form.hidden ? "primary" : "";
      if (!form.hidden) { const first = form.querySelector("input, select, textarea"); if (first) first.focus(); }
    });
    tools.append(toggle);
  }
  const refresh = el("button", "Refresh"); refresh.type = "button";
  refresh.addEventListener("click", () => render());
  tools.append(refresh);
  root.append(tools);
