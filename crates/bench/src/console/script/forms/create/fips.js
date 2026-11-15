  if (current === "fips") {
    const wrap = el("div");
    const name = el("input"); name.required = true; name.placeholder = "demo";
    const network = el("input"); network.required = true; network.placeholder = "fleet";
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("External network", network));
    form.append(el("strong", "New floating IP"), row);
    const go = el("button", "Create floating IP"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      go.disabled = true;
      try {
        const net = await api("GET", "/network/networks/" + encodeURIComponent(network.value.trim()));
        await api("POST", "/network/floating-ips", { name: name.value.trim(), spec: { network: net.id } });
        name.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      go.disabled = false;
    });
    const assoc = el("form"); assoc.className = "create"; assoc.autocomplete = "off";
    const fipName = el("input"); fipName.required = true; fipName.placeholder = "demo";
    const machName = el("input"); machName.required = true; machName.placeholder = "demo";
    const arow = el("div"); arow.className = "row";
    arow.append(field("Floating IP", fipName), field("Machine", machName));
    const ago = el("button", "Associate"); ago.className = "primary"; ago.type = "submit";
    assoc.append(el("strong", "Associate"), arow, ago);
    assoc.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      ago.disabled = true;
      try {
        const fip = await api("GET", "/network/floating-ips/" + encodeURIComponent(fipName.value.trim()));
        const mach = await api("GET", "/compute/machines/" + encodeURIComponent(machName.value.trim()));
        await api("POST", "/network/floating-ips/" + encodeURIComponent(fip.id) + "/associate", { machine: mach.id });
        await render();
      } catch (e) { setErr(e.message); }
      ago.disabled = false;
    });
    wrap.append(form, assoc);
    return wrap;
  }
