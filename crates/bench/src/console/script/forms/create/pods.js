  if (current === "pods") {
    const name = el("input"); name.required = true; name.placeholder = "web";
    const container = el("input"); container.required = true; container.placeholder = "web";
    const image = el("input"); image.required = true; image.placeholder = "nginx:1.27";
    const network = el("input"); network.placeholder = "default";
    form.append(el("strong", "New pod"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Container", container), field("Image", image), field("Network", network));
    form.append(row);
    const go = el("button", "Create pod"); go.className = "primary"; go.type = "submit";
    form.append(go);
    onCreate(form, go, async () => {
      const spec = { containers: [{ name: container.value.trim(), image: image.value.trim() }] };
      const net = network.value.trim();
      if (net) spec.networks = [net];
      await api("POST", "/loom/pods", { name: name.value.trim(), spec });
      name.value = ""; container.value = ""; image.value = ""; network.value = "";
    });
    return form;
  }
