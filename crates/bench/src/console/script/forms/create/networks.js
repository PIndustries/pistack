  if (current === "networks") {
    const name = el("input"); name.required = true;
    form.append(field("Network name", name));
    const go = el("button", "Create network"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      try {
        await api("POST", "/network/networks", { name: name.value.trim(), spec: { kind: { type: "overlay" }, mtu: 1500, external: false } });
        name.value = "";
        await render();
      } catch (e) { setErr(e.message); }
    });
    return form;
  }
