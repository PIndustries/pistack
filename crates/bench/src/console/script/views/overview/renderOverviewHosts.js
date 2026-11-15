
  const right = el("div");
  right.append(el("h3", "Hosts"));
  const hp = el("div", null, "panel");
  if (!hosts.length) {
    hp.append(el("p", "No host has joined yet. Start an agent with pistack agent --join."));
  } else {
    const t = simpleTable([
      ["Name", i => i.name],
      ["Phase", i => i.status && i.status.phase],
      ["vCPU", () => ""],
      ["Memory", () => ""],
    ], hosts.slice(0, 12));
    [...t.querySelectorAll("tbody tr")].forEach((tr, idx) => {
      const h = hosts[idx];
      const cap = (h.spec && h.spec.capacity) || {};
      const free = (h.status && h.status.free) || {};
      const cpu = (cap.vcpus || 0) - (free.vcpus || 0);
      const mem = (cap.memory_mib || 0) - (free.memory_mib || 0);
      tr.children[2].replaceChildren(meter(cpu, cap.vcpus || 0), cpu + " / " + (cap.vcpus || 0));
      tr.children[3].replaceChildren(meter(mem, cap.memory_mib || 0), gib(mem) + " / " + gib(cap.memory_mib || 0) + " GiB");
    });
    hp.append(t);
  }
  right.append(hp);
  split.append(left, right);
  root.append(split);
