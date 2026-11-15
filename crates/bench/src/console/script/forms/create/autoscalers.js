  if (current === "autoscalers") {
    const name = el("input"); name.required = true; name.placeholder = "web-scale";
    const targetKind = el("select");
    targetKind.append(option("Deployment", "Deployment"), option("StatefulSet", "StatefulSet"));
    const target = el("input"); target.required = true; target.placeholder = "web";
    const metric = el("input"); metric.required = true; metric.placeholder = "cpu.usage";
    const targetValue = el("input"); targetValue.type = "number"; targetValue.step = "any"; targetValue.required = true; targetValue.value = "70";
    const minR = el("input"); minR.type = "number"; minR.min = "1"; minR.value = "1"; minR.required = true;
    const maxR = el("input"); maxR.type = "number"; maxR.min = "1"; maxR.value = "5"; maxR.required = true;
    form.append(el("strong", "New autoscaler"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Target kind", targetKind), field("Target", target), field("Metric", metric), field("Target value", targetValue), field("Min", minR), field("Max", maxR));
    form.append(row);
    const go = el("button", "Create autoscaler"); go.className = "primary"; go.type = "submit";
    form.append(go);
    onCreate(form, go, async () => {
      const t = Number(String(targetValue.value).trim());
      if (!Number.isFinite(t)) throw new Error("target value must be a finite number");
      const min = intField(minR.value, "min", 1);
      const max = intField(maxR.value, "max", 1);
      if (max < min) throw new Error("max must be at least min");
      await api("POST", "/loom/autoscalers", {
        name: name.value.trim(),
        spec: {
          target_kind: targetKind.value,
          target_name: target.value.trim(),
          metric: metric.value.trim(),
          target_value: t,
          min_replicas: min,
          max_replicas: max,
        },
      });
      name.value = ""; target.value = ""; metric.value = "";
    });
    return form;
  }
