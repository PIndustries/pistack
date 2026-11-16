
  // What can be picked, read once when the form is built.
  (async () => {
    const list = async (path) => itemsOf(await api("GET", path));
    try {
      const [images, sizes, networks, groups, hosts] = await Promise.all([
        list("/image/images"), list("/compute/sizes"), list("/network/networks"),
        list("/network/security-groups").catch(() => []), list("/compute/hosts").catch(() => []),
      ]);
      const zoneHosts = new Map();
      for (const h of hosts) {
        const z = h.labels && h.labels.zone;
        if (z && !(h.spec && h.spec.disabled)) zoneHosts.set(z, (zoneHosts.get(z) || 0) + 1);
      }
      sizes.sort((a, b) => ((a.spec.vcpus || 0) - (b.spec.vcpus || 0)) || ((a.spec.memory_mib || 0) - (b.spec.memory_mib || 0)));
      data = { images, sizes, networks, groups, zones: [...zoneHosts].map(([name, n]) => ({ name, hosts: n })).sort((a, b) => a.name.localeCompare(b.name)) };
      // One of each to start from, where there is only one sensible pick.
      const ready = images.filter(i => phaseOf(i) === "ready");
      if (ready.length === 1) state.image.add(ready[0].name);
      const inside = networks.filter(n => !(n.spec && n.spec.external));
      if (inside.length === 1) state.networks.add(inside[0].name);
    } catch (e) {
      failed = "Could not read what this project can use: " + e.message;
    }
    draw();
  })();
  draw();
  return form;
}

