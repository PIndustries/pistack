  if (current === "services") {
    const name = el("input"); name.required = true; name.placeholder = "web";
    const key = el("input"); key.placeholder = "app";
    const value = el("input"); value.placeholder = "web";
    const port = el("input"); port.required = true; port.value = "80"; port.inputMode = "numeric";
    const kind = el("select");
    kind.append(option("ClusterIp", "cluster IP"), option("NodePort", "node port"), option("LoadBalancer", "load balancer"));
    form.append(el("strong", "New service"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Selector key", key), field("Selector value", value), field("Port", port), field("Kind", kind));
    form.append(row);
    const go = el("button", "Create service"); go.className = "primary"; go.type = "submit";
    form.append(go);
    onCreate(form, go, async () => {
      const spec = { ports: [{ port: intField(port.value, "port", 1, 65535) }], kind: kind.value };
      const k = key.value.trim();
      const v = value.value.trim();
      if (k || v) {
        if (!k || !v) throw new Error("selector needs both a key and a value");
        spec.selector = { [k]: v };
      }
      await api("POST", "/loom/services", { name: name.value.trim(), spec });
      name.value = ""; key.value = ""; value.value = "";
    });
    return form;
  }
