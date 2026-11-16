  if (current === "shares") {
    const name = el("input"); name.required = true; name.placeholder = "home";
    const protocol = el("select");
    protocol.append(option("nfs"), option("cephfs"));
    const size = el("input"); size.type = "number"; size.min = "1"; size.value = "10"; size.required = true;
    const cidr = el("input"); cidr.placeholder = "10.0.0.0/24";
    const mode = el("select");
    mode.append(option("read"), option("write"));
    form.append(el("strong", "New share"));
    const row = el("div"); row.className = "row";
    row.append(field("Name", name), field("Protocol", protocol), field("Size GiB", size), field("Client CIDR", cidr), field("Mode", mode));
    form.append(row);
    const go = el("button", "Create share"); go.className = "primary"; go.type = "submit";
    form.append(go);
    onCreate(form, go, async () => {
      const spec = { protocol: protocol.value, size_gib: intField(size.value, "size", 1) };
      const client = cidr.value.trim();
      if (client) spec.access = [{ cidr: client, mode: mode.value }];
      await api("POST", "/shelf/shares", { name: name.value.trim(), spec });
      name.value = ""; cidr.value = "";
    });
    return form;
  }
