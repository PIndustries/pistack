  if (current === "dns") {
    const name = el("input"); name.required = true; name.placeholder = "lab";
    const apex = el("input"); apex.required = true; apex.placeholder = "lab.cloud.";
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Apex", apex));
    form.append(el("strong", "New zone"), row);
    const go = el("button", "Create zone"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      go.disabled = true;
      try {
        await api("POST", "/dns/zones", {
          name: name.value.trim(),
          labels: { "pistack.io/auto": "true" },
          spec: { name: apex.value.trim(), kind: { type: "primary" } },
        });
        name.value = "";
        apex.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      go.disabled = false;
    });
    return form;
  }
