  if (current === "keys") {
    const name = el("input"); name.required = true; name.placeholder = "laptop"; name.autocomplete = "off";
    form.append(field("Name", name));
    const go = el("button", "Create key"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      go.disabled = true;
      try {
        const data = await api("POST", "/identity/keys", { name: name.value.trim() });
        keyOnce = data && data.secret ? String(data.secret) : "";
        name.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      go.disabled = false;
    });
    return form;
  }
