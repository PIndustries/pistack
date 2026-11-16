  const draw = () => {
    const step = steps[at];
    heading.textContent = step.title;
    lead.textContent = step.lead;
    pane.replaceChildren();
    if (!data && step.key !== "details" && step.key !== "access") {
      pane.append(el("div", failed || "Reading what this project can use…", "sub"));
      refresh();
      return;
    }
    if (step.key === "details") {
      const row = el("div"); row.className = "row";
      row.append(field("Name", name), field("How many", count));
      pane.append(row);
      const zones = data ? data.zones : [];
      if (zones.length) {
        pane.append(el("h3", "Availability zone"));
        pane.append(optionCards(
          [{ value: "", title: "Any zone", detail: "wherever there is room" }]
            .concat(zones.map(z => ({ value: z.name, title: z.name, detail: z.hosts + (z.hosts === 1 ? " host" : " hosts") }))),
          state.zone, false, refresh, ""));
      }
    } else if (step.key === "image") {
      pane.append(optionCards(data.images.map(i => {
        const ready = phaseOf(i) === "ready";
        return { value: i.name, title: i.name, disabled: !ready, why: "This image is not ready",
          detail: [(i.spec && i.spec.format) || "", (i.spec && i.spec.arch) || "", ready ? "" : phaseOf(i)].filter(Boolean).join(" · ") };
      }), state.image, false, refresh, "No image yet. Upload one on the Images page first."));
    } else if (step.key === "size") {
      pane.append(optionCards(data.sizes.map(s => ({ value: s.name, title: s.name, detail: sizeText(s) })),
        state.size, false, refresh, "No size is defined. An admin adds them with pi size create."));
    } else if (step.key === "networks") {
      pane.append(optionCards(data.networks.map(n => ({
        value: n.name, title: n.name,
        detail: n.spec && n.spec.external ? "external · addresses come from outside" : ((n.spec && n.spec.kind && n.spec.kind.type) || "network"),
      })), state.networks, true, refresh, "No network yet. Create one on the Networks page, with a subnet."));
      pane.append(el("p", "Name a network to boot on that project's bridge. A router on that network NATs the guest. The address on the port is the address the guest uses.", "sub"));
      pane.append(el("p", "A machine gets no floating IP on its own. If an admin has marked an external network for automatic floating IPs, the guest gets one from it when it has an address, and the auto DNS zone publishes its name.", "sub"));
    } else if (step.key === "groups") {
      pane.append(optionCards(data.groups.map(g => ({
        value: g.name, title: g.name,
        detail: (((g.spec && g.spec.rules) || []).length) + " rules",
      })), state.groups, true, refresh, "No security group yet. The project's default group will be used."));
    } else if (step.key === "access") {
      pane.append(field("SSH public key", key));
      pane.append(el("p", "Paste the public half. It is put in the default user's authorized keys, and kept in this browser for next time.", "sub"));
      pane.append(field("User data (cloud-init)", userData));
    } else {
      const kv = el("div", null, "kv review");
      const size = data.sizes.find(s => s.name === one(state.size));
      kv.append(
        kvLine("Name", name.value.trim() + (howMany() > 1 ? "-1 … " + name.value.trim() + "-" + howMany() : "")),
        kvLine("Image", one(state.image)),
        kvLine("Size", one(state.size) ? one(state.size) + " — " + sizeText(size) : ""),
        kvLine("Zone", one(state.zone) || "any"),
        kvLine("Networks", [...state.networks].join(", ") || "none: it will have no address"),
        kvLine("Security groups", [...state.groups].join(", ") || "the project's default group"),
        kvLine("SSH key", key.value.trim() ? key.value.trim().split(/\s+/).slice(-1)[0].slice(0, 40) : "none: you will need the console to get in"),
        kvLine("User data", userData.value.trim() ? userData.value.trim().split("\n").length + " lines" : "none"),
      );
      pane.append(kv);
      const lacks = missing();
      if (lacks.length) {
        pane.append(el("p", "Still needed: " + lacks.map(k => steps.find(s => s.key === k).title.toLowerCase()).join(", ") + ".", "sub"));
      }
    }
    refresh();
  };
