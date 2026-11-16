  if (current === "routers") {
    const name = el("input"); name.required = true;
    const network = el("input"); network.placeholder = "optional";
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("External network", network));
    form.append(el("strong", "New router"), row);
    const go = el("button", "Create router"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      go.disabled = true;
      try {
        const spec = {};
        const ext = network.value.trim();
        if (ext) {
          const net = await api("GET", "/network/networks/" + encodeURIComponent(ext));
          spec.external_network = net.id;
        }
        await api("POST", "/network/routers", { name: name.value.trim(), spec });
        name.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      go.disabled = false;
    });
    return form;
  }
