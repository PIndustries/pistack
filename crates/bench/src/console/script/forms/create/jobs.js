  if (current === "jobs") {
    const name = el("input"); name.required = true; name.placeholder = "batch";
    const container = el("input"); container.placeholder = "same as name";
    const image = el("input"); image.required = true; image.placeholder = "busybox";
    const completions = el("input"); completions.type = "number"; completions.min = "1"; completions.value = "1"; completions.required = true;
    const parallelism = el("input"); parallelism.type = "number"; parallelism.min = "1"; parallelism.value = "1"; parallelism.required = true;
    const policy = el("select");
    policy.append(option("Never", "never"), option("OnFailure", "on failure"));
    form.append(el("strong", "New job"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Container", container), field("Image", image), field("Completions", completions), field("Parallelism", parallelism), field("Restart", policy));
    form.append(row);
    const go = el("button", "Create job"); go.className = "primary"; go.type = "submit";
    form.append(go);
    onCreate(form, go, async () => {
      const n = name.value.trim();
      await api("POST", "/loom/jobs", {
        name: n,
        spec: {
          completions: intField(completions.value, "completions", 1),
          parallelism: intField(parallelism.value, "parallelism", 1),
          template: {
            spec: {
              restart_policy: policy.value,
              containers: [{ name: container.value.trim() || n, image: image.value.trim() }],
            },
          },
        },
      });
      name.value = ""; container.value = ""; image.value = "";
    });
    return form;
  }
