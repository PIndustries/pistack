  if (current === "statefulsets") {
    const name = el("input"); name.required = true; name.placeholder = "db";
    const replicas = el("input"); replicas.type = "number"; replicas.min = "0"; replicas.value = "1"; replicas.required = true;
    const container = el("input"); container.placeholder = "same as name";
    const image = el("input"); image.required = true; image.placeholder = "postgres:16";
    const service = el("input"); service.placeholder = "optional";
    form.append(el("strong", "New StatefulSet"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Replicas", replicas), field("Container", container), field("Image", image), field("Service", service));
    form.append(row);
    const go = el("button", "Create StatefulSet"); go.className = "primary"; go.type = "submit";
    form.append(go);
    onCreate(form, go, async () => {
      const n = name.value.trim();
      const spec = {
        replicas: intField(replicas.value, "replicas", 0),
        template: {
          labels: { app: n },
          spec: { containers: [{ name: container.value.trim() || n, image: image.value.trim() }] },
        },
      };
      const svc = service.value.trim();
      if (svc) spec.service_name = svc;
      await api("POST", "/loom/statefulsets", { name: n, spec });
      name.value = ""; container.value = ""; image.value = ""; replicas.value = "1"; service.value = "";
    });
    return form;
  }
