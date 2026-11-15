  // ---- capacity ----
  root.append(el("h3", "Capacity"));
  const cap = el("div", null, "kv");
  const capLine = (label, use, unit) => {
    const p = el("p");
    p.append(el("span", label, "k"));
    const v = el("span", null, "v");
    v.append(meter(use[0], use[1]), " " + use[0] + " of " + use[1] + (unit ? " " + unit : "") + " used");
    p.append(v);
    return p;
  };
  cap.append(
    capLine("vCPU", vcpuUse(host), ""),
    capLine("Memory", gibUse(host), "GiB"),
    capLine("Disk", diskUse(host), "GiB"),
  );
  root.append(cap);
  root.append(el("p", "Capacity less what the scheduler has placed here. The agent's own free figures are what the charts show.", "sub"));

