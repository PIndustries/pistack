  if (current === "projects") {
    const name = el("input"); name.required = true;
    const desc = el("input");
    const enabled = el("select");
    enabled.append(option("true", "yes"), option("false", "no"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Description", desc), field("Enabled", enabled));
    form.append(el("strong", "New project"), row);
    const go = el("button", "Create project"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      go.disabled = true;
      try {
        const body = { name: name.value.trim(), enabled: enabled.value === "true" };
        const d = desc.value.trim();
        if (d) body.description = d;
        await api("POST", "/identity/projects", body);
        name.value = ""; desc.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      go.disabled = false;
    });
    return form;
  }
