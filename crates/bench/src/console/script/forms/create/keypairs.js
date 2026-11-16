  if (current === "keypairs") {
    const name = el("input"); name.required = true; name.placeholder = "laptop"; name.autocomplete = "off";
    const publicKey = el("textarea"); publicKey.rows = 3;
    publicKey.placeholder = "ssh-ed25519 AAAA… leave empty to make a new pair";
    form.append(field("Name", name), field("Public key", publicKey));
    const go = el("button", "Create key pair"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      go.disabled = true;
      try {
        const body = { name: name.value.trim() };
        const line = publicKey.value.trim();
        if (line) body.public_key = line;
        const data = await api("POST", "/compute/keypairs", body);
        keypairOnce = data && data.private_key ? String(data.private_key) : null;
        name.value = "";
        publicKey.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      go.disabled = false;
    });
    return form;
  }
