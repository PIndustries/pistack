  if (current === "users") {
    const name = el("input"); name.required = true; name.autocomplete = "off";
    const kind = el("select");
    kind.append(option("user", "user"), option("service_account", "service account"));
    const email = el("input"); email.type = "email"; email.autocomplete = "off";
    const password = el("input");
    password.type = "password";
    password.autocomplete = "new-password";
    password.spellcheck = false;
    const enabled = el("select");
    enabled.append(option("true", "yes"), option("false", "no"));
    form.append(el("strong", "New user"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Kind", kind), field("Email", email), field("Enabled", enabled));
    form.append(row, field("Password", password));
    const go = el("button", "Create user"); go.className = "primary"; go.type = "submit";
    form.append(go);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      setErr("");
      go.disabled = true;
      try {
        const body = { name: name.value.trim(), kind: kind.value, enabled: enabled.value === "true" };
        const em = email.value.trim();
        if (em) body.email = em;
        const pw = password.value;
        if (pw) body.password = pw;
        await api("POST", "/identity/principals", body);
        password.value = "";
        name.value = "";
        email.value = "";
        await render();
      } catch (e) { setErr(e.message); }
      go.disabled = false;
    });
    return form;
  }
