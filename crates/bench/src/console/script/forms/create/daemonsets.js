  if (current === "daemonsets") {
    const name = el("input"); name.required = true; name.placeholder = "agent";
    const container = el("input"); container.placeholder = "same as name";
    const image = el("input"); image.required = true; image.placeholder = "busybox";
    form.append(el("strong", "New DaemonSet"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Container", container), field("Image", image));
    form.append(row);
    const go = el("button", "Create DaemonSet"); go.className = "primary"; go.type = "submit";
    form.append(go);
    onCreate(form, go, async () => {
      const n = name.value.trim();
      await api("POST", "/loom/daemonsets", {
        name: n,
        spec: {
          template: {
            labels: { app: n },
            spec: { containers: [{ name: container.value.trim() || n, image: image.value.trim() }] },
          },
        },
      });
      name.value = ""; container.value = ""; image.value = "";
    });
    return form;
  }
