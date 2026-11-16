  if (current === "volumes") {
    const wrap = el("div");
    const name = el("input"); name.required = true;
    const size = el("input"); size.type = "number"; size.min = "1"; size.value = "10";
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Size GiB", size));
    form.append(el("strong", "New volume"), row);
    const go = el("button", "Create volume"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      go.disabled = true;
      try {
        await api("POST", "/volume/volumes", { name: name.value.trim(), spec: { size_gib: Number(size.value) } });
        name.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      go.disabled = false;
    });
    const attach = el("form"); attach.className = "create"; attach.autocomplete = "off";
    const volName = el("input"); volName.required = true; volName.placeholder = "data";
    const machName = el("input"); machName.required = true; machName.placeholder = "demo";
    const device = el("input"); device.placeholder = "vdb";
    const arow = el("div"); arow.className = "row";
    arow.append(field("Volume", volName), field("Machine", machName), field("Device", device));
    const attachGo = el("button", "Attach volume"); attachGo.className = "primary"; attachGo.type = "submit";
    const detachGo = el("button", "Detach volume"); detachGo.type = "button";
    attach.append(el("strong", "Attach"), arow, attachGo, document.createTextNode(" "), detachGo);
    attach.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      attachGo.disabled = true;
      try {
        const vol = await api("GET", "/volume/volumes/" + encodeURIComponent(volName.value.trim()));
        const mach = await api("GET", "/compute/machines/" + encodeURIComponent(machName.value.trim()));
        const body = { machine: mach.id };
        const dev = device.value.trim();
        if (dev) body.device = dev;
        await api("POST", "/volume/volumes/" + vol.id + "/attach", body);
        await render();
      } catch (e) { setErr(e.message); }
      attachGo.disabled = false;
    });
    detachGo.addEventListener("click", async () => {
      setErr("");
      detachGo.disabled = true;
      try {
        const vol = await api("GET", "/volume/volumes/" + encodeURIComponent(volName.value.trim()));
        await api("POST", "/volume/volumes/" + vol.id + "/detach");
        await render();
      } catch (e) { setErr(e.message); }
      detachGo.disabled = false;
    });
    const snap = el("form"); snap.className = "create"; snap.autocomplete = "off";
    const snapName = el("input"); snapName.required = true; snapName.placeholder = "data-1";
    const snapVol = el("input"); snapVol.required = true; snapVol.placeholder = "data";
    const srow = el("div"); srow.className = "row";
    srow.append(field("Snapshot name", snapName), field("Volume name", snapVol));
    const snapGo = el("button", "Create snapshot"); snapGo.className = "primary"; snapGo.type = "submit";
    snap.append(el("strong", "New snapshot"), srow, snapGo);
    snap.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      snapGo.disabled = true;
      try {
        const vol = await api("GET", "/volume/volumes/" + encodeURIComponent(snapVol.value.trim()));
        await api("POST", "/volume/snapshots", { name: snapName.value.trim(), spec: { volume: vol.id } });
        snapName.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      snapGo.disabled = false;
    });
    wrap.append(form, attach, snap);
    return wrap;
  }
