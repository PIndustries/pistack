  if (current === "ports") {
    const name = el("input"); name.required = true;
    const network = el("input"); network.required = true; network.placeholder = "fleet";
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Network", network));
    form.append(el("strong", "New port"), row);
    const go = el("button", "Create port"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      go.disabled = true;
      try {
        const net = await api("GET", "/network/networks/" + encodeURIComponent(network.value.trim()));
        await api("POST", "/network/ports", { name: name.value.trim(), spec: { network: net.id } });
        name.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      go.disabled = false;
    });
    return form;
  }
