  if (current === "alarms") {
    const name = el("input"); name.required = true; name.placeholder = "cpu-high";
    const metric = el("input"); metric.required = true; metric.placeholder = "cpu.usage";
    const comparison = el("select");
    comparison.append(option("gt", "greater than"), option("lt", "less than"), option("eq", "equal"));
    const threshold = el("input"); threshold.type = "number"; threshold.step = "any"; threshold.required = true; threshold.value = "0";
    const seconds = el("input"); seconds.type = "number"; seconds.min = "0"; seconds.value = "0"; seconds.required = true;
    form.append(el("strong", "New alarm"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Metric", metric), field("Comparison", comparison), field("Threshold", threshold), field("For seconds", seconds));
    form.append(row);
    const go = el("button", "Create alarm"); go.className = "primary"; go.type = "submit";
    form.append(go);
    onCreate(form, go, async () => {
      const n = name.value.trim();
      const t = Number(String(threshold.value).trim());
      if (!Number.isFinite(t)) throw new Error("threshold must be a finite number");
      await api("POST", "/gauge/alarms", {
        name: n,
        spec: {
          name: n,
          metric: metric.value.trim(),
          comparison: comparison.value,
          threshold: t,
          for_seconds: intField(seconds.value, "for seconds", 0),
        },
      });
      name.value = ""; metric.value = "";
    });
    return form;
  }
