  if (current === "deployments") {
    const name = el("input"); name.required = true; name.placeholder = "web";
    const replicas = el("input"); replicas.type = "number"; replicas.min = "0"; replicas.value = "1"; replicas.required = true;
    const container = el("input"); container.placeholder = "same as name";
    const image = el("input"); image.required = true; image.placeholder = "nginx:1.27";
    form.append(el("strong", "New deployment"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Replicas", replicas), field("Container", container), field("Image", image));
    form.append(row);
    const go = el("button", "Create deployment"); go.className = "primary"; go.type = "submit";
    form.append(go);
    onCreate(form, go, async () => {
      const n = name.value.trim();
      await api("POST", "/loom/deployments", {
        name: n,
        spec: {
          replicas: intField(replicas.value, "replicas", 0),
          template: {
            labels: { app: n },
            spec: { containers: [{ name: container.value.trim() || n, image: image.value.trim() }] },
          },
        },
      });
      name.value = ""; container.value = ""; image.value = ""; replicas.value = "1";
    });
    return form;
  }
