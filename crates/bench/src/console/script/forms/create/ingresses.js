  if (current === "ingresses") {
    const name = el("input"); name.required = true; name.placeholder = "web";
    const host = el("input"); host.placeholder = "web.example.com";
    const path = el("input"); path.required = true; path.value = "/";
    const service = el("input"); service.required = true; service.placeholder = "web";
    const port = el("input"); port.required = true; port.value = "80"; port.inputMode = "numeric";
    form.append(el("strong", "New ingress"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Host", host), field("Path", path), field("Service", service), field("Port", port));
    form.append(row);
    const go = el("button", "Create ingress"); go.className = "primary"; go.type = "submit";
    form.append(go);
    onCreate(form, go, async () => {
      const p = path.value.trim() || "/";
      if (p.charAt(0) !== "/") throw new Error("path must start with /");
      const rule = {
        paths: [{
          path: p,
          backend: { service: service.value.trim(), port: intField(port.value, "port", 1, 65535) },
        }],
      };
      const h = host.value.trim();
      if (h) rule.host = h;
      await api("POST", "/loom/ingresses", { name: name.value.trim(), spec: { rules: [rule] } });
      name.value = ""; host.value = ""; service.value = ""; path.value = "/";
    });
    return form;
  }
